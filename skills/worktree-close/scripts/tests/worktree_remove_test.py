"""Exercise removal against real Git worktrees and a controlled Bus CLI boundary."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "worktree_remove.py"


class WorktreeRemoveTest(unittest.TestCase):
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
        self.state = {"ok": True, "result": {"agents": []}, "error": None}
        self.exit_code = 0

    @staticmethod
    def git(*args):
        return subprocess.run(["git", *args], check=True, capture_output=True, text=True)

    def agent(self, agent_id=12, name="reviewer", cwd=None, **fields):
        return {"id": agent_id, "name": name, "cwd": str(cwd or self.target),
                "status": "idle", "deletion_pending": False, **fields}

    def run_helper(self, *args, session_cwd=None, missing_bus=False):
        if not missing_bus:
            cli = self.bin / "bus"
            cli.write_text(
                f"#!{sys.executable}\nimport sys\n"
                "assert sys.argv[1:] == ['state']\n"
                f"print({json.dumps(self.state)!r})\nsys.exit({self.exit_code})\n"
            )
            cli.chmod(0o755)
        # Supply only the controlled Bus and real Git; never contact the live session.
        env = {**os.environ, "PATH": str(self.bin)}
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--repo", str(self.repo), "--worktree",
             str(self.target), "--session-cwd", str(session_cwd or self.repo), *args],
            cwd=self.repo, env=env, capture_output=True, text=True,
        )

    def assert_refused(self, result):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertTrue(self.target.is_dir(), result.stdout + result.stderr)

    def test_idle_agent_launch_dir_blocks_force_removal_from_main(self):
        self.state["result"]["agents"] = [self.agent()]
        result = self.run_helper("--force")
        self.assert_refused(result)
        self.assertIn("reviewer (12)", result.stderr)
        for action in ("non-destructive cleanup", "final result", "orchestrator", "delete",
                       "unaffected session"):
            self.assertIn(action, result.stderr)

    def test_all_statuses_and_pending_deletion_remain_holders(self):
        self.state["result"]["agents"] = [
            self.agent(14, "working", status="working"),
            self.agent(11, "blocked", status="blocked"),
            self.agent(13, "unavailable", status="unavailable"),
            self.agent(10, "retiring", deletion_pending=True),
        ]
        result = self.run_helper()
        self.assert_refused(result)
        self.assertLess(result.stderr.index("retiring (10)"), result.stderr.index("working (14)"))
        for name in ("working (14)", "blocked (11)", "unavailable (13)"):
            self.assertIn(name, result.stderr)

    def test_resolved_ancestry_finds_symlinked_nested_launch_dir(self):
        child = self.target / "nested"
        child.mkdir()
        alias = self.root / "alias"
        alias.symlink_to(self.target, target_is_directory=True)
        self.state["result"]["agents"] = [self.agent(cwd=alias / "nested")]
        self.assert_refused(self.run_helper())

    def test_symlinked_target_is_also_protected(self):
        alias = self.root / "alias"
        alias.symlink_to(self.target, target_is_directory=True)
        self.state["result"]["agents"] = [self.agent()]
        self.target = alias
        self.assert_refused(self.run_helper())

    def test_path_prefix_sibling_is_not_a_holder(self):
        self.state["result"]["agents"] = [self.agent(cwd=self.root / "feature-other")]
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.target.exists())

    def test_force_still_removes_authorized_dirty_worktree_without_holders(self):
        (self.target / "discard.txt").write_text("authorized discard")
        result = self.run_helper("--force")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.target.exists())

    def test_removal_refreshes_roster_after_a_successful_preflight(self):
        result = self.run_helper("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.state["result"]["agents"] = [self.agent()]
        self.assert_refused(self.run_helper())

    def test_empty_roster_check_is_non_destructive(self):
        result = self.run_helper("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.target.is_dir())

    def test_launch_session_guard_survives_bus_being_absent(self):
        result = self.run_helper(session_cwd=self.target / "nested", missing_bus=True)
        self.assert_refused(result)
        self.assertIn("session", result.stderr)

    def test_missing_bus_warns_and_preserves_previous_removal_behavior(self):
        result = self.run_helper(missing_bus=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("WARNING", result.stderr)
        self.assertFalse(self.target.exists())

    def test_missing_bus_does_not_force_discard_unsaved_work(self):
        (self.target / "keep.txt").write_text("uncommitted work")
        result = self.run_helper(missing_bus=True)
        self.assert_refused(result)
        self.assertIn("WARNING", result.stderr)
        self.assertTrue((self.target / "keep.txt").exists())

    def test_unreachable_bus_warns_and_allows_removal(self):
        self.state = {"ok": False, "result": None,
                      "error": {"code": "control_unavailable", "message": "Bus is not running"}}
        self.exit_code = 1
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("WARNING", result.stderr)
        self.assertFalse(self.target.exists())

    def test_reachable_malformed_state_fails_closed(self):
        malformed = [
            {}, {"ok": True, "result": {}},
            {"ok": True, "result": {"agents": None}},
            {"ok": True, "result": {"agents": [self.agent(cwd="relative")]}},
            {"ok": True, "result": {"agents": [{"id": 12, "name": "reviewer"}]}},
            {"ok": True, "result": {"agents": [self.agent(), self.agent()]}},
            {"ok": True, "result": {"agents": [self.agent(agent_id=True)]}},
        ]
        for state in malformed:
            with self.subTest(state=state):
                self.state = state
                result = self.run_helper("--check")
                self.assert_refused(result)
                self.assertNotIn("WARNING", result.stderr)

    def test_reachable_bus_command_error_is_not_unreachable(self):
        self.state = {"ok": False, "result": None,
                      "error": {"code": "invalid_arguments", "message": "version mismatch"}}
        self.exit_code = 1
        self.assert_refused(self.run_helper())

    def test_main_checkout_cannot_be_removed(self):
        self.target = self.repo
        self.assert_refused(self.run_helper("--force"))

    def test_git_rejection_is_reported_without_force_retry(self):
        (self.target / "keep.txt").write_text("uncommitted work")
        result = self.run_helper()
        self.assert_refused(result)
        self.assertTrue((self.target / "keep.txt").exists())


if __name__ == "__main__":
    unittest.main()
