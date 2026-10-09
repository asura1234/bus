"""Disposable binary fixtures; never stop, launch, or install a real Bus."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

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
watch_processes() { [[ "$1" != wait || "$FIXTURE_OUTCOME" != wait_failed ]]; }
resume_session() {
    trap - EXIT
    printf 'MOCK_RESUME=%s\\n' "${1:-$bus_cutover_binary}"
    exit 0
}
main
'''
        candidate = self.candidate if outcome != "stale_candidate" else installed
        result = subprocess.run(
            ["bash", "-c", body, "cutover-test", str(repo / "tools/cutover.sh")],
            env={**os.environ, "XDG_DATA_HOME": str(self.root / "data"),
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
        self.assertFalse((backup / "session").exists())
        self.assertIn("MOCK_RESUME=", result.stdout)

    def test_stale_candidate_is_rejected_before_requesting_a_stop(self):
        result, installed, old, backup = self.run_cutover("stale_candidate")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(installed.read_bytes(), old)
        self.assertNotIn("Step 2:", result.stdout)
        self.assertNotIn("MOCK_RESUME=", result.stdout)
        self.assertIn("status=not_installed", (backup / "cutover-status.txt").read_text())


if __name__ == "__main__":
    unittest.main()
