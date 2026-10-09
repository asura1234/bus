"""review-pr's prologue reviews the committed diff with plan documents excluded."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
REVIEW_ROUND_SCRIPT = REPO_ROOT / "skills/review-pr/scripts/review_round.py"
# The prologue's lane / round / ledger part shares one implementation under cli_extensions with
# /review-plan. The fixture copies the script into a synthetic repository to isolate git state, so that
# shared module must come along too — in the real repository the script always sits inside REPO_ROOT,
# so the dependency holds naturally.
REVIEW_ROUND_COMMON = REPO_ROOT / "cli_extensions/review_round_common.py"
PR_GOAL_CONTEXT = REPO_ROOT / "skills/pr/scripts/pr_goal_context.py"


def _run(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, cwd=repo, capture_output=True, text=True, check=False)


class ReviewPrRoundTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.repo = self._create_review_repo(Path(temporary.name))

    def _create_review_repo(self, tmp_path: Path) -> Path:
        repo = tmp_path / "repo"
        script = repo / "skills/review-pr/scripts/review_round.py"
        script.parent.mkdir(parents=True)
        shutil.copy2(REVIEW_ROUND_SCRIPT, script)
        shared = repo / "cli_extensions/review_round_common.py"
        shared.parent.mkdir(parents=True)
        shutil.copy2(REVIEW_ROUND_COMMON, shared)
        shutil.copy2(REPO_ROOT / "cli_extensions/review_scope.py", shared.parent / "review_scope.py")
        pr = repo / "skills/pr/scripts/pr_goal_context.py"
        pr.parent.mkdir(parents=True)
        shutil.copy2(PR_GOAL_CONTEXT, pr)

        for args in (
            ("git", "init", "-b", "main"),
            ("git", "config", "user.name", "Review Test"),
            ("git", "config", "user.email", "review-test@example.com"),
        ):
            self.assertEqual(_run(repo, *args).returncode, 0)
        (repo / "README.md").write_text("base\n")
        self.assertEqual(_run(repo, "git", "add", "README.md").returncode, 0)
        self.assertEqual(_run(repo, "git", "commit", "-m", "base").returncode, 0)
        self.assertEqual(_run(repo, "git", "switch", "-c", "feat/review-test").returncode, 0)

        goal = repo / "temp/review-pr/feat-review-test/.locked-goal"
        goal.parent.mkdir(parents=True)
        goal.write_text("验证 review-pr 范围\n")
        (goal.parent / ".locked-non-goals").write_text("无\n")
        return repo

    def _commit(self, *paths: str) -> None:
        self.assertEqual(_run(self.repo, "git", "add", *paths).returncode, 0)
        self.assertEqual(_run(self.repo, "git", "commit", "-m", "change").returncode, 0)

    def _review_round(self) -> subprocess.CompletedProcess[str]:
        return _run(
            self.repo,
            sys.executable,
            "skills/review-pr/scripts/review_round.py",
            "--base",
            "main",
            "--reviewer",
            "test",
        )

    def test_review_snapshot_excludes_plan_changes(self) -> None:
        plan = self.repo / "plans/example.md"
        plan.parent.mkdir(parents=True)
        plan.write_text("plan change\n")
        (self.repo / "source.txt").write_text("code change\n")
        self._commit("plans/example.md", "source.txt")

        result = self._review_round()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        snapshot = (
            self.repo / "temp/review-pr/feat-review-test/test/round-01/diff-snapshot.patch"
        ).read_text()
        self.assertIn("source.txt", snapshot)
        self.assertNotIn("plans/example.md", snapshot)

    def test_review_round_rejects_plan_only_changes(self) -> None:
        plan = self.repo / "plans/example.md"
        plan.parent.mkdir(parents=True)
        plan.write_text("plan change\n")
        self._commit("plans/example.md")

        result = self._review_round()

        self.assertEqual(result.returncode, 1)
        self.assertIn("排除 plans 后没有可审查的已提交变更", result.stdout)

    def test_branch_incremental_round_keeps_diff_snapshot_semantics(self) -> None:
        source = self.repo / "source.txt"
        source.write_text("first version\n")
        self._commit("source.txt")
        first = self._review_round()
        self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
        state = self.repo / "temp/review-pr/feat-review-test/test"
        (state / "round-01/review.md").write_text("completed review\n")
        source.write_text("author fix\n")
        self._commit("source.txt")
        (self.repo / "README.md").write_text("uncommitted work\n")
        second = self._review_round()
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertIn("MODE=incremental", second.stdout)
        self.assertNotIn("SCOPE_HASH=", second.stdout)
        snapshot = (state / "round-02/diff-snapshot.patch").read_text()
        expected = _run(self.repo, "git", "diff", "main...HEAD", "--", ".", ":(exclude)plans/**").stdout
        self.assertEqual(snapshot, expected)
        delta = (state / "round-02/diff-delta.patch").read_text()
        self.assertTrue(delta.startswith("--- round-01/diff-snapshot.patch\n+++ round-02/diff-snapshot.patch\n"))
        self.assertNotIn("uncommitted work", snapshot + delta)


if __name__ == "__main__":
    unittest.main()
