"""Refuse live launch directories reached through filesystem case aliases."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "worktree_remove.py"


class WorktreeRemoveCaseAliasTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "main"
        self.target = self.root / "feature"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        (self.bin / "git").symlink_to(shutil.which("git"))
        self.git("init", "-q", str(self.repo))
        self.git("-C", str(self.repo), "-c", "user.name=Test", "-c",
                 "user.email=test@example.com", "commit", "-qm", "initial", "--allow-empty")
        self.git("-C", str(self.repo), "worktree", "add", "-qb", "feature", str(self.target))
        self.alias = self.target.with_name("FEATURE")
        self.case_insensitive = self.alias.exists() and self.alias.samefile(self.target)

    def require_case_alias(self):
        if not self.case_insensitive:
            self.skipTest("requires a case-insensitive filesystem")

    @staticmethod
    def git(*args):
        return subprocess.run(["git", *args], check=True, capture_output=True, text=True)

    def run_helper(self, *, target=None, holder=None, session=None, missing_bus=False):
        if not missing_bus:
            state = {"ok": True, "result": {"agents": []}, "error": None}
            if holder is not None:
                state["result"]["agents"] = [
                    {"id": 12, "name": "reviewer", "cwd": str(holder), "status": "idle"}
                ]
            bus = self.bin / "bus"
            bus.write_text(f"#!{sys.executable}\nprint({json.dumps(state)!r})\n")
            bus.chmod(0o755)
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("BUS_", "HERDR_"))}
        env["PATH"] = str(self.bin)
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--repo", str(self.repo), "--worktree",
             str(target or self.target), "--session-cwd", str(session or self.repo), "--force"],
            cwd=self.repo, env=env, capture_output=True, text=True,
        )

    def assert_protected(self, result):
        detail = f"target exists: {self.target.is_dir()}; helper exit: {result.returncode}\n"
        detail += result.stdout + result.stderr
        self.assertTrue(self.target.is_dir(), detail)
        self.assertEqual(result.returncode, 1, detail)

    def test_idle_holder_with_case_alias_is_protected(self):
        self.require_case_alias()
        self.assert_protected(self.run_helper(holder=self.alias))

    def test_target_with_case_alias_protects_idle_holder(self):
        self.require_case_alias()
        self.assert_protected(self.run_helper(target=self.alias, holder=self.target))

    def test_session_with_case_alias_is_protected_when_bus_absent(self):
        self.require_case_alias()
        self.assert_protected(self.run_helper(session=self.alias, missing_bus=True))

    def test_missing_nested_launch_dir_under_case_alias_is_protected(self):
        self.require_case_alias()
        self.assert_protected(self.run_helper(holder=self.alias / "missing-child"))

    def test_main_checkout_case_alias_is_rejected_before_git(self):
        self.require_case_alias()
        result = self.run_helper(target=self.repo.with_name("MAIN"))
        self.assert_protected(result)
        self.assertIn("Cannot remove the main worktree", result.stderr)

    def test_distinct_case_sensitive_sibling_does_not_block_removal(self):
        if self.case_insensitive:
            self.skipTest("requires a case-sensitive filesystem")
        self.alias.mkdir()
        result = self.run_helper(holder=self.alias)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.target.exists())
        self.assertTrue(self.alias.is_dir())


if __name__ == "__main__":
    unittest.main()
