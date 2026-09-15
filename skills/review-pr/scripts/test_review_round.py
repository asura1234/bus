"""Direct prologue tests for review-pr's two locked Goal/Non-goals inputs."""

import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))

from review_round import (  # noqa: E402
    LockedContextError,
    load_locked_review_context,
)


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


if __name__ == "__main__":
    unittest.main()
