"""Installed skills must execute the target repository's gate without Bus tooling."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]


class GenericGateTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "target"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "trunk")
        self.git("config", "user.name", "Gate Test")
        self.git("config", "user.email", "gate@example.test")
        (self.repo / ".gitignore").write_text("temp/\n")
        (self.repo / "file").write_text("before\n")
        self.git("add", ".gitignore", "file")
        self.git("commit", "-qm", "initial")
        self.base = self.git("rev-parse", "HEAD")
        self.git("checkout", "-qb", "feature")
        (self.repo / "file").write_text("after\n")
        self.git("commit", "-qam", "feature")
        self.bundle = self.root / "installed skills"
        shutil.copytree(SCRIPTS, self.bundle / "gate-and-fix/scripts")
        self.runner = self.bundle / "gate-and-fix/scripts/gate_and_fix.py"
        self.scanner = self.bundle / "worktree-new/scripts/repo_profile.py"
        self.scanner.parent.mkdir(parents=True)
        # A local scanner executable represents the producer's documented stdout contract.
        self.scanner.write_text(
            "import json, pathlib, sys\n"
            "assert sys.argv[1:3] == ['scan', '--repo']\n"
            "repo = pathlib.Path(sys.argv[3])\n"
            "print((repo / 'policy.json').read_text())\n"
        )
        shutil.copytree(SCRIPTS.parents[1] / "rebase-origin-main/scripts", self.bundle / "rebase-origin-main/scripts")
        self.profile(None)

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], text=True).strip()

    def profile(self, command):
        (self.repo / "policy.json").write_text(json.dumps({
            "policy": {"gate_command": command, "require_exact_head_gate": True}
        }))
        self.git("add", "policy.json")
        self.git("commit", "-qm", "gate policy")

    def run_gate(self, *args, env_extra=None):
        nested = self.repo / "nested"
        nested.mkdir(exist_ok=True)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("BUS_", "HERDR_"))}
        env.update(env_extra or {})
        return subprocess.run([
            sys.executable, str(self.runner), "--base", self.base, "--round", "1",
            "--artifact-root", str(self.repo / "temp/evidence"), *args
        ], cwd=nested, env=env, text=True, capture_output=True)

    def test_profile_command_runs_in_repo_root_and_logs_pass(self):
        self.profile("pwd; printf 'fake gate stdout\\n'; printf 'fake stderr\\n' >&2")
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        artifact = Path(result.stdout.strip()).read_text()
        self.assertIn("### repository-gate — PASS", artifact)
        self.assertNotIn("### coverage", artifact)
        decoded = subprocess.run([
            sys.executable, str(self.runner), "show", "--artifact", result.stdout.strip(),
            "--base", self.base, "--gate", "repository-gate", "--stream", "stdout"
        ], text=True, capture_output=True, check=True)
        self.assertEqual(decoded.stdout, f"{self.repo}\nfake gate stdout\n")
        self.assertFalse((self.repo / "temp/gate-and-fix/coverage").exists())

    def test_failing_gate_preserves_exit_and_logs(self):
        self.profile("printf 'failed fake gate\\n'; exit 7")
        result = self.run_gate()
        self.assertEqual(result.returncode, 1, result.stderr)
        artifact = Path(result.stdout.strip()).read_text()
        self.assertIn("### repository-gate — FAIL", artifact)
        self.assertIn("- Exit code: `7`", artifact)

    def test_no_gate_is_reported_without_pass_artifact(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("no gate configured", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_explicit_command_works_without_scanner(self):
        self.scanner.unlink()
        result = self.run_gate("--command", "printf 'caller gate\\n'")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("repository-gate", Path(result.stdout.strip()).read_text())

    def test_missing_scanner_without_command_never_passes(self):
        self.scanner.unlink()
        result = self.run_gate()
        self.assertEqual(result.returncode, 2)
        self.assertIn("no gate configured", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_malformed_profile_is_not_masked_by_command(self):
        (self.repo / "policy.json").write_text('{"policy": {"gate_command": 42, "require_exact_head_gate": true}}')
        self.git("commit", "-qam", "malformed policy")
        result = self.run_gate("--command", "true")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertIn("gate_command must be a nonempty string or null", result.stderr)

    def test_explicit_just_ci_does_not_opt_non_bus_profile_into_bus_lanes(self):
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        just = fake_bin / "just"
        just.write_text("#!/bin/sh\nprintf 'external just gate\\n'\n")
        just.chmod(0o755)
        from unittest.mock import patch
        with patch.dict(os.environ, {"PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}"}):
            result = self.run_gate("--command", "just ci")
        self.assertEqual(result.returncode, 0, result.stderr)
        artifact = Path(result.stdout.strip()).read_text()
        self.assertIn("### repository-gate — PASS", artifact)
        self.assertNotIn("### lint", artifact)

    def test_failed_scan_is_not_masked_by_command(self):
        self.scanner.write_text("raise SystemExit('scanner failure')\n")
        result = self.run_gate("--command", "true")
        self.assertEqual(result.returncode, 2)
        self.assertIn("repo profile scan failed", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_shell_commands_with_backticks_and_newlines_roundtrip(self):
        for command, expected in [("printf '%s' `printf shell-gate`", "shell-gate"),
                                  ("printf first\nprintf second", "firstsecond")]:
            with self.subTest(command=command):
                self.profile(command)
                result = self.run_gate()
                self.assertEqual(result.returncode, 0, result.stderr)
                shown = subprocess.run([
                    sys.executable, str(self.runner), "show", "--artifact", result.stdout.strip(),
                    "--base", self.base, "--gate", "repository-gate", "--stream", "stdout"
                ], text=True, capture_output=True, check=True)
                self.assertEqual(shown.stdout, expected)

    def test_generic_gate_scrubs_live_session_env_but_keeps_target_env(self):
        self.profile('printf "%s" "${BUS_DATA_DIR-absent}:${HERDR_COORDINATE-absent}:${TARGET_GATE_VALUE}"')
        result = self.run_gate(env_extra={"BUS_DATA_DIR": "/isolated/sentinel", "HERDR_COORDINATE": "sentinel", "TARGET_GATE_VALUE": "kept"})
        self.assertEqual(result.returncode, 0, result.stderr)
        shown = subprocess.run([
            sys.executable, str(self.runner), "show", "--artifact", result.stdout.strip(),
            "--base", self.base, "--gate", "repository-gate", "--stream", "stdout"
        ], text=True, capture_output=True, check=True)
        self.assertEqual(shown.stdout, "absent:absent:kept")

    def test_unignored_evidence_is_relocated_to_git_metadata(self):
        (self.repo / ".gitignore").write_text("")
        self.git("commit", "-qam", "no temporary directory convention")
        self.profile("printf clean-gate")
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("status", "--porcelain", "--untracked-files=all"), "")
        self.assertTrue(Path(result.stdout.strip()).is_relative_to(self.repo / ".git"))

    def test_real_scanner_keeps_unignored_linked_worktree_and_main_clean(self):
        (self.repo / ".gitignore").write_text("")
        policy = self.repo / ".bus/repo.toml"
        policy.parent.mkdir()
        policy.write_text('gate_command = "printf scanned-gate"\n')
        self.git("add", ".bus/repo.toml", ".gitignore")
        self.git("commit", "-qm", "real policy and no temporary directory ignore")
        scanner = SCRIPTS.parents[1] / "worktree-new/scripts/repo_profile.py"
        shutil.copyfile(scanner, self.scanner)
        linked = self.root / "linked"
        self.git("worktree", "add", "-qb", "linked-feature", str(linked))
        result = self.run_gate("--repo", str(linked))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("status", "--porcelain", "--untracked-files=all"), "")
        self.assertEqual(subprocess.check_output(["git", "-C", str(linked), "status", "--porcelain", "--untracked-files=all"], text=True), "")
        self.assertFalse((self.repo / "temp").exists())
        self.assertFalse((linked / "temp").exists())

    def test_gate_rejects_origin_default_branch_even_with_explicit_base(self):
        self.profile("printf should-not-run")
        self.git("update-ref", "refs/remotes/origin/trunk", self.git("rev-parse", "HEAD"))
        self.git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
        self.git("checkout", "-q", "trunk")
        self.git("merge", "--ff-only", "feature")
        result = self.run_gate()
        self.assertEqual(result.returncode, 2)
        self.assertIn("protected branch", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_gate_edit_invalidates_passing_exit(self):
        self.profile("printf changed > file")
        result = self.run_gate()
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("changed while gates ran", result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
