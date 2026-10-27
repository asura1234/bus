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
        self.profile(None)

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], text=True).strip()

    def profile(self, command):
        (self.repo / "policy.json").write_text(json.dumps({
            "policy": {"gate_command": command, "require_exact_head_gate": True}
        }))
        self.git("add", "policy.json")
        self.git("commit", "-qm", "gate policy")

    def run_gate(self, *args):
        nested = self.repo / "nested"
        nested.mkdir(exist_ok=True)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("BUS_", "HERDR_"))}
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
        (self.repo / "policy.json").write_text('{"policy": {"gate_command": 42}}')
        self.git("commit", "-qam", "malformed policy")
        result = self.run_gate("--command", "true")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")

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

    def test_gate_edit_invalidates_passing_exit(self):
        self.profile("printf changed > file")
        result = self.run_gate()
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("changed while gates ran", result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
