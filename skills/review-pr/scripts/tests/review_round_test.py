"""Direct prologue tests for review-pr's two locked Goal/Non-goals inputs."""

import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from review_round import (  # noqa: E402
    LockedContextError,
    load_locked_review_context,
)
from review_scope import ReviewScopeError, load_review_scope  # noqa: E402


REPO_ROOT = Path(__file__).resolve().parents[4]


PLAN = """# 示例

## 目标

交付房间编排内容。
- 保留 **Markdown**。

## 非目标

- 不修改 harness。
"""


class LockedReviewContextTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.lane_root = self.root / "temp" / "review-pr" / "feat-room"
        self.lane_root.mkdir(parents=True)
        self.plan = self.root / "plan.md"
        self.plan.write_text(PLAN, encoding="utf-8")

    def write_locks(self, goal: str | None, non_goals: str | None) -> None:
        for name, value in ((".locked-goal", goal), (".locked-non-goals", non_goals)):
            path = self.lane_root / name
            if value is None:
                path.unlink(missing_ok=True)
            else:
                path.write_text(value, encoding="utf-8")

    def test_planless_review_reads_both_locks(self) -> None:
        self.write_locks("单一目标\n", "无\n")
        context = load_locked_review_context(self.lane_root, None)
        self.assertEqual((context.goal, context.non_goals), ("单一目标", "无"))
        self.assertEqual(context.goal_file, self.lane_root / ".locked-goal")
        self.assertEqual(context.non_goals_file, self.lane_root / ".locked-non-goals")

    def test_missing_goal_lock_fails_closed(self) -> None:
        self.write_locks(None, "无\n")
        with self.assertRaisesRegex(LockedContextError, r"\.locked-goal"):
            load_locked_review_context(self.lane_root, None)

    def test_missing_non_goals_lock_fails_closed(self) -> None:
        self.write_locks("单一目标\n", None)
        with self.assertRaisesRegex(LockedContextError, r"\.locked-non-goals"):
            load_locked_review_context(self.lane_root, None)

    def test_blank_locks_fail_closed(self) -> None:
        for goal, non_goals, expected in (
            (" \n", "无\n", r"\.locked-goal"),
            ("单一目标\n", "\n\t\n", r"\.locked-non-goals"),
        ):
            with self.subTest(expected=expected):
                self.write_locks(goal, non_goals)
                with self.assertRaisesRegex(LockedContextError, expected):
                    load_locked_review_context(self.lane_root, None)

    def test_plan_bound_review_accepts_exact_matching_locks(self) -> None:
        self.write_locks(
            "交付房间编排内容。\n- 保留 **Markdown**。\n",
            "- 不修改 harness。\n",
        )
        context = load_locked_review_context(self.lane_root, self.plan)
        self.assertEqual(context.goal, "交付房间编排内容。\n- 保留 **Markdown**。")
        self.assertEqual(context.non_goals, "- 不修改 harness。")

    def test_plan_goal_mismatch_fails_closed(self) -> None:
        self.write_locks("另一个目标\n", "- 不修改 harness。\n")
        with self.assertRaisesRegex(LockedContextError, "Goal.*不一致"):
            load_locked_review_context(self.lane_root, self.plan)

    def test_plan_non_goals_mismatch_fails_closed(self) -> None:
        self.write_locks("交付房间编排内容。\n- 保留 **Markdown**。\n", "无\n")
        with self.assertRaisesRegex(LockedContextError, "Non-goals.*不一致"):
            load_locked_review_context(self.lane_root, self.plan)

    def test_plan_without_required_sections_fails_closed(self) -> None:
        self.write_locks("单一目标\n", "无\n")
        self.plan.write_text("## 目标\n\n单一目标\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "非目标"):
            load_locked_review_context(self.lane_root, self.plan)


class ScopeRoundTests(unittest.TestCase):
    """Exercise the documented entry point against real, isolated committed Git trees."""

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in (
            "skills/review-pr/scripts/review_round.py",
            "skills/pr/scripts/pr_goal_context.py",
            "cli_extensions/review_round_common.py",
            "cli_extensions/review_scope.py",
        ):
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO_ROOT / name, target)
        self.run_command("git", "init", "-b", "main")
        self.run_command("git", "config", "user.name", "Review Test")
        self.run_command("git", "config", "user.email", "review-test@example.com")
        (self.root / "source.py").write_text("first = 1\nsecond = 2\nthird = 3\n")
        (self.root / "source_test.py").write_text("def test_source(): pass\n")
        (self.root / "other.py").write_text("outside = 1\n")
        self.commit("source.py", "source_test.py", "other.py")
        self.run_command("git", "switch", "-c", "feat/chunk")
        locks = self.root / "temp/review-pr/feat-chunk"
        locks.mkdir(parents=True)
        (locks / ".locked-goal").write_text("审查指定生产 chunk 的全部代码。\n")
        (locks / ".locked-non-goals").write_text("无\n")
        self.manifest = self.root / "chunk.json"
        self.manifest.write_text(json.dumps({"files": ["source.py"], "test_files": ["source_test.py"]}))

    def run_command(self, *args: str) -> subprocess.CompletedProcess:
        result = subprocess.run(args, cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def commit(self, *paths: str) -> None:
        self.run_command("git", "add", "--", *paths)
        self.run_command("git", "commit", "-m", "fixture")

    def prologue(self, *args: str, success: bool = True) -> tuple[subprocess.CompletedProcess, dict[str, str]]:
        result = subprocess.run(
            [sys.executable, "skills/review-pr/scripts/review_round.py", "--base", "main", "--scope", str(self.manifest), *args],
            cwd=self.root, capture_output=True, text=True,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result, dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)

    def finish(self, output: dict[str, str]) -> None:
        (self.root / output["STATE_DIR"] / f"round-{int(output['ROUND']):02d}" / "review.md").write_text("completed review\n")

    def test_round_one_reads_unchanged_whole_committed_files(self) -> None:
        (self.root / "source.py").write_text("dirty source\n")
        _, output = self.prologue("--reviewer", "test")
        self.assertEqual(output["MODE"], "full")
        self.assertEqual(output["TARGET_KIND"], "scope")
        patch = (self.root / output["DIFF_SNAPSHOT"]).read_text()
        self.assertIn("+first = 1\n+second = 2\n+third = 3", patch)
        self.assertIn("source_test.py", patch)
        self.assertNotIn("dirty source", patch)
        self.assertNotIn("other.py", patch)
        self.assertEqual(output["SCOPE_TEST_FILES"], "source_test.py")

    def test_round_two_uses_only_chunk_delta_and_prior_lane(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        (self.root / "source.py").write_text("first = 1\nsecond = 20\nthird = 3\n")
        (self.root / "other.py").write_text("outside = 2\n")
        self.commit("source.py", "other.py")
        _, second = self.prologue("--reviewer", "test", "--devils-advocate")
        self.assertEqual(second["MODE"], "incremental")
        self.assertEqual(second["ROUND"], "2")
        self.assertIn("test/round-01/review.md", second["PREV_REVIEWS"])
        patch = (self.root / second["DIFF_DELTA"]).read_text()
        self.assertIn("-second = 2\n+second = 20", patch)
        self.assertNotIn("source_test.py", patch)
        self.assertNotIn("other.py", patch)

    def test_dirty_reviewer_probe_does_not_become_author_delta(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        (self.root / "source_test.py").write_text("def test_source(): assert False\n")
        _, second = self.prologue("--reviewer", "test")
        self.assertEqual((self.root / second["DIFF_DELTA"]).read_text(), "")

    def test_new_declared_probe_enters_delta_only_when_adopted(self) -> None:
        self.manifest.write_text(json.dumps({"files": ["source.py"], "test_files": ["new_test.py"]}))
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        (self.root / "new_test.py").write_text("def test_regression(): pass\n")
        _, dirty = self.prologue("--reviewer", "test")
        self.assertEqual((self.root / dirty["DIFF_DELTA"]).read_text(), "")
        self.commit("new_test.py")
        _, adopted = self.prologue("--reviewer", "test")
        self.assertIn("+def test_regression()", (self.root / adopted["DIFF_DELTA"]).read_text())

    def test_deletion_keeps_fixed_target_and_reports_delta(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        self.run_command("git", "rm", "source.py")
        self.run_command("git", "commit", "-m", "delete fixture")
        _, second = self.prologue("--reviewer", "test")
        self.assertEqual(second["SCOPE_HASH"], first["SCOPE_HASH"])
        self.assertIn("deleted file mode", (self.root / second["DIFF_DELTA"]).read_text())

    def test_entire_chunk_deletion_is_still_an_incremental_delta(self) -> None:
        self.manifest.write_text("source.py\n")
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        self.run_command("git", "rm", "source.py")
        self.run_command("git", "commit", "-m", "delete whole chunk")
        _, second = self.prologue("--reviewer", "test")
        self.assertEqual(second["MODE"], "incremental")
        self.assertIn("-first = 1", (self.root / second["DIFF_DELTA"]).read_text())

    def test_rebase_with_identical_chunk_is_empty_delta(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        self.run_command("git", "commit", "--allow-empty", "-m", "different head")
        _, second = self.prologue("--reviewer", "test")
        self.assertNotEqual(first["HEAD"], second["HEAD"])
        self.assertEqual((self.root / second["DIFF_DELTA"]).read_text(), "")

    def test_scope_and_lanes_and_branch_history_are_isolated(self) -> None:
        branch_history = self.root / "temp/review-pr/feat-chunk/test/round-01"
        branch_history.mkdir(parents=True)
        (branch_history / "review.md").write_text("branch review")
        _, first = self.prologue("--reviewer", "test")
        self.assertEqual(first["MODE"], "full")
        self.finish(first)
        _, other_lane = self.prologue("--reviewer", "other")
        self.assertEqual(other_lane["MODE"], "full")
        self.manifest.write_text("other.py\n")
        _, other_chunk = self.prologue("--reviewer", "test")
        self.assertEqual(other_chunk["MODE"], "full")
        self.assertNotEqual(other_chunk["SCOPE_HASH"], first["SCOPE_HASH"])

    def test_bare_round_continuation_and_named_lane_ambiguity(self) -> None:
        _, first = self.prologue()
        self.finish(first)
        _, second = self.prologue()
        self.assertEqual(second["MODE"], "incremental")
        self.finish(second)
        _, named = self.prologue("--reviewer", "named")
        self.finish(named)
        result, _ = self.prologue(success=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("lane", result.stdout)

    def test_chunk_ledgers_do_not_mix_with_branch_or_other_chunks(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        root = self.root / "temp/address-review-comments/feat-chunk"
        for suffix in ("branch", f"scopes/{first['SCOPE_HASH']}/one", "scopes/other/two"):
            ledger = root / suffix / "triage.md"
            ledger.parent.mkdir(parents=True)
            ledger.write_text("**模式**：pr\n\n## APPLY\n无。\n")
        _, current = self.prologue("--reviewer", "test")
        self.assertIn(f"scopes/{first['SCOPE_HASH']}/one/triage.md", current["TRIAGE_LEDGER"])
        self.assertNotIn("branch/triage", current["TRIAGE_LEDGER"])
        self.assertNotIn("scopes/other", current["TRIAGE_LEDGER"])

    def test_missing_incremental_snapshot_fails_closed(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        (self.root / first["SCOPE_SNAPSHOT"]).unlink()
        result, _ = self.prologue("--reviewer", "test", success=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("前轮 scope snapshot", result.stdout)

    def test_corrupt_incremental_snapshot_fails_without_traceback(self) -> None:
        _, first = self.prologue("--reviewer", "test")
        self.finish(first)
        (self.root / first["SCOPE_SNAPSHOT"]).write_text('{"head":"' + "a" * 40 + '","files":[]}')
        result, _ = self.prologue("--reviewer", "test", success=False)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("Traceback", result.stderr)
        self.assertIn("前轮 scope snapshot", result.stdout)

    def test_missing_source_and_plan_targets_fail(self) -> None:
        for value in ("missing.py\n", "plans/example.md\n"):
            self.manifest.write_text(value)
            result, _ = self.prologue("--reviewer", "test", success=False)
            self.assertNotEqual(result.returncode, 0)

    def test_canonical_manifest_identity_and_test_write_allowlist(self) -> None:
        first = load_review_scope(self.manifest, self.root)
        self.manifest.write_text("# same chunk\nsource_test.py\n./source.py\nsource.py\n")
        second = load_review_scope(self.manifest, self.root)
        self.assertEqual(first.scope_hash, second.scope_hash)
        self.manifest.write_text(json.dumps({"files": ["source.py"], "test_files": []}))
        self.assertNotEqual(first.scope_hash, load_review_scope(self.manifest, self.root).scope_hash)

    def test_invalid_paths_and_production_probe_permissions_fail(self) -> None:
        for value in (
            {"files": []}, {"files": ["../source.py"]}, {"files": ["source.py"], "test_files": ["source.py"]},
            {"files": ["source.py"], "test_files": ["docs_test.md"]},
            {"files": ["source.py"], "extra": []}, {"files": ["temp/probe_test.py"]},
            {"files": ["docs/plans/example.md"]},
        ):
            with self.subTest(value=value):
                self.manifest.write_text(json.dumps(value))
                with self.assertRaises(ReviewScopeError):
                    load_review_scope(self.manifest, self.root)

    def test_symlink_and_binary_targets_fail(self) -> None:
        alias = self.root / "alias.py"
        alias.symlink_to(self.root / "source.py")
        self.manifest.write_text("alias.py\n")
        result, _ = self.prologue("--reviewer", "test", success=False)
        self.assertNotEqual(result.returncode, 0)
        (self.root / "binary.bin").write_bytes(b"a\0b")
        self.commit("binary.bin")
        self.manifest.write_text("binary.bin\n")
        result, _ = self.prologue("--reviewer", "test", success=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("二进制", result.stdout)


if __name__ == "__main__":
    unittest.main()
