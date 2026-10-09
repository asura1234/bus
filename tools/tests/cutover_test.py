"""Disposable binary fixtures; never stop, launch, or install a real Bus."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import cutover


@unittest.skipUnless(os.name == "posix", "executable fixtures require POSIX")
class CutoverTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="bus-cutover-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.head = "6" * 40
        self.candidate = self.binary("candidate", self.head)
        self.manifest = self.root / "candidate.json"

    def binary(self, name, head, suffix=""):
        binary = self.root / name
        binary.write_text(f"#!/bin/sh\nprintf '%s\\n' 'bus 0.9.0-dev.{head}'\n{suffix}")
        binary.chmod(0o700)
        return binary

    def test_old_binary_cannot_be_recorded_as_the_new_commit(self):
        old = self.binary("old", "b" * 40)
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            cutover.record(old, self.head, self.manifest)
        self.assertFalse(self.manifest.exists())

    def test_rollback_cannot_pass_installed_candidate_verification(self):
        cutover.record(self.candidate, self.head, self.manifest)
        rollback = self.binary("installed", "b" * 40)
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            cutover.verify(rollback, self.manifest)

    def test_same_revision_but_different_bytes_cannot_pass_installation(self):
        cutover.record(self.candidate, self.head, self.manifest)
        different = self.binary("installed", self.head, "# another build\n")
        with self.assertRaisesRegex(ValueError, "differs from the verified candidate"):
            cutover.verify(different, self.manifest)

    def test_exact_installed_copy_reports_the_verified_revision_and_digest(self):
        receipt = cutover.record(self.candidate, self.head, self.manifest)
        installed = self.root / "installed"
        installed.write_bytes(self.candidate.read_bytes())
        installed.chmod(0o700)
        self.assertEqual(cutover.verify(installed, self.manifest)["sha256"], receipt["sha256"])

    def test_unlabelled_binary_fails_closed(self):
        self.candidate.write_text("#!/bin/sh\nprintf 'bus 0.9.0\\n'\n")
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            cutover.record(self.candidate, self.head, self.manifest)

    def test_backup_copies_lock_files_and_symlinks_but_excludes_runtime_sockets(self):
        source = self.root / "session"
        source.mkdir()
        (source / "state.json").write_text("saved state")
        (source / "coordinator.lock").touch()
        (source / "state-link").symlink_to("state.json")
        socket_path = source / "dev-control.sock"
        socket_path.touch()
        real_lstat = Path.lstat

        def lstat(path):
            if path == socket_path:
                return os.stat_result((0o140600, 0, 0, 1, 0, 0, 0, 0, 0, 0))
            return real_lstat(path)

        with patch.object(Path, "lstat", lstat):
            cutover.backup(source, self.root / "backup")
        self.assertEqual((self.root / "backup/state.json").read_text(), "saved state")
        self.assertTrue((self.root / "backup/coordinator.lock").exists())
        self.assertTrue((self.root / "backup/state-link").is_symlink())
        self.assertFalse((self.root / "backup/dev-control.sock").exists())

    def test_wait_does_not_reinspect_removed_socket_paths(self):
        self.manifest.write_text('{"pids":{"123":"old"},"groups":{"123":"old"}}')
        with patch.object(cutover, "processes", return_value={}), \
                patch.object(cutover, "session_roots", side_effect=AssertionError("removed sockets")):
            cutover.wait(self.manifest, timeout=0)

    def test_wait_timeout_preserves_orphan_process_diagnostics(self):
        self.manifest.write_text('{"pids":{"123":"old"},"groups":{"123":"old"}}')
        with patch.object(cutover, "processes", return_value={123: (1, 123, "old", "helper")}):
            with self.assertRaisesRegex(RuntimeError, "PID 123: PPID=1 PGID=123.*command=helper"):
                cutover.wait(self.manifest, timeout=0)
        self.assertIn("helper", self.manifest.with_suffix(".remaining.json").read_text())

    def test_reused_group_leader_does_not_adopt_unrelated_session(self):
        watched, groups = {123: "old"}, {123: "old"}
        rows = {123: (1, 123, "new", "other-bus"), 124: (123, 123, "new", "other-agent")}
        self.assertEqual(cutover.extend(rows, watched, groups, set()), set())
        self.assertNotIn(124, watched)

    def test_default_session_root_matches_bus_even_when_xdg_data_home_is_set(self):
        result = subprocess.run(
            ["bash", "-c", 'source "$1" 0123456789abcdef; printf "%s" "$bus_cutover_session_dir"',
             "cutover-test", str(Path(cutover.__file__).with_suffix(".sh"))],
            env={key: value for key, value in {**os.environ, "XDG_DATA_HOME": str(self.root)}.items()
                 if key != "BUS_CUTOVER_SESSION_DIR"},
            capture_output=True, text=True, timeout=5,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, str(Path.home() / ".local/share/bus/sessions/0123456789abcdef"))

    def run_cutover(self, outcome):
        repo = self.root / "repo"
        (repo / "tools").mkdir(parents=True)
        original = Path(cutover.__file__).parent
        for name in ["cutover.sh", "cutover.py"]:
            shutil.copy2(original / name, repo / "tools" / name)
        installed = repo / "target/debug/bus"
        installed.parent.mkdir(parents=True)
        installed.write_text("#!/bin/sh\nprintf '{\"stopped\":true}\\n'\n")
        installed.chmod(0o700)
        old = installed.read_bytes()
        session = self.root / "data/bus/sessions/0123456789abcdef"
        session.mkdir(parents=True)
        (session / "state.json").write_text('{"fixture":true}\n')
        body = '''source "$1" 0123456789abcdef
preflight() { bus_cutover_verified_head=$FIXTURE_HEAD; }
build_candidate() { mkdir -p "$1/debug"; cp "$FIXTURE_CANDIDATE" "$1/debug/bus"; : >"$2"; }
if [[ "$FIXTURE_OUTCOME" == build_failed ]]; then
    build_candidate() { printf 'fixture compiler failed\\n' >&2; return 19; }
fi
watch_processes() {
    if [[ "$1" == wait && "$FIXTURE_OUTCOME" == wait_failed ]]; then
        printf 'Timed out waiting for session processes: PID 123 helper\\n' >&2
        return 17
    fi
}
if [[ "$FIXTURE_OUTCOME" == backup_failed ]]; then
    backup_session() { printf 'fixture copy: Permission denied\\n' >&2; return 23; }
fi
if [[ "$FIXTURE_OUTCOME" != resume_failed ]]; then
    resume_session() {
        trap - EXIT
        printf 'MOCK_RESUME=%s\\n' "${1:-$bus_cutover_binary}"
        exit 0
    }
fi
main
'''
        candidate = self.candidate if outcome != "stale_candidate" else installed
        if outcome == "resume_failed":
            candidate.write_text(f'''#!/bin/sh
if [ "$1" = --version ]; then
    printf '%s\\n' 'bus 0.9.0-dev.{self.head}'
else
    printf 'fixture resume failed\\n' >&2
    exit 31
fi
''')
        result = subprocess.run(
            ["bash", "-c", body, "cutover-test", str(repo / "tools/cutover.sh")],
            env={**os.environ, "BUS_CUTOVER_SESSION_DIR": str(session),
                 "FIXTURE_HEAD": self.head, "FIXTURE_CANDIDATE": str(candidate),
                 "FIXTURE_OUTCOME": outcome},
            capture_output=True, text=True, timeout=15,
        )
        backup = Path(next(line.split(": ", 1)[1] for line in result.stdout.splitlines()
                           if line.startswith("Backup and build-log directory: ")))
        self.addCleanup(shutil.rmtree, backup)
        return result, installed, old, backup

    def test_success_records_installed_revision_before_resuming(self):
        result, installed, _, backup = self.run_cutover("success")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(installed.read_bytes(), self.candidate.read_bytes())
        self.assertIn("status=installed_verified", (backup / "cutover-status.txt").read_text())
        self.assertIn(self.head, result.stdout)
        self.assertIn("MOCK_RESUME=", result.stdout)

    def test_wait_failure_resumes_old_binary_and_records_failed_deployment(self):
        result, installed, old, backup = self.run_cutover("wait_failed")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(installed.read_bytes(), old)
        self.assertIn("NEW BUILD WAS NOT DEPLOYED", result.stderr)
        self.assertIn("status=rolled_back", (backup / "cutover-status.txt").read_text())
        status = (backup / "cutover-status.txt").read_text()
        self.assertIn("step=wait_for_exit", status)
        self.assertIn("exit_code=17", status)
        self.assertIn("PID 123 helper", status)
        self.assertIn("PID 123 helper", (backup / "cutover.log").read_text())
        self.assertFalse((backup / "session").exists())
        self.assertIn("MOCK_RESUME=", result.stdout)

    def test_backup_failure_records_its_step_and_exact_stderr_before_rollback(self):
        result, installed, old, backup = self.run_cutover("backup_failed")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(installed.read_bytes(), old)
        status = (backup / "cutover-status.txt").read_text()
        self.assertIn("step=backup_session", status)
        self.assertIn("exit_code=23", status)
        self.assertIn("fixture copy: Permission denied", status)
        self.assertIn("fixture copy: Permission denied", (backup / "cutover.log").read_text())

    def test_stale_candidate_is_rejected_before_requesting_a_stop(self):
        result, installed, old, backup = self.run_cutover("stale_candidate")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(installed.read_bytes(), old)
        self.assertNotIn("Step 2:", result.stdout)
        self.assertNotIn("MOCK_RESUME=", result.stdout)
        self.assertIn("status=not_installed", (backup / "cutover-status.txt").read_text())

    def test_build_failure_records_error_without_requesting_stop(self):
        result, installed, old, backup = self.run_cutover("build_failed")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(installed.read_bytes(), old)
        self.assertNotIn("Step 2:", result.stdout)
        status = (backup / "cutover-status.txt").read_text()
        self.assertIn("step=build_candidate", status)
        self.assertIn("exit_code=19", status)
        self.assertIn("fixture compiler failed", status)

    def test_resume_failure_records_the_handoff_error(self):
        result, _, _, backup = self.run_cutover("resume_failed")
        self.assertEqual(result.returncode, 31, result.stderr)
        status = (backup / "cutover-status.txt").read_text()
        self.assertIn("status=resume_failed", status)
        self.assertIn("step=resume_session", status)
        self.assertIn("exit_code=31", status)
        self.assertIn("fixture resume failed", status)
        self.assertIn("fixture resume failed", (backup / "cutover.log").read_text())


if __name__ == "__main__":
    unittest.main()
