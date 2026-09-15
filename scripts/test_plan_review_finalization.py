"""Hash-bound plan-review finalization by an explicitly assigned separate author."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]


def _load_finalizer():
    path = REPO / "skills" / "address-review-comments" / "scripts" / "finalize_plan_review.py"
    spec = importlib.util.spec_from_file_location("finalize_plan_review", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


finalizer = _load_finalizer()


def plan_text(status: str, body: str = "交付示例计划") -> str:
    return f"# 示例计划\n\n**状态**：{status}\n\n## 目标\n\n{body}\n"


class PlanReviewFinalizationTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.plan = self.root / "plans" / "example.md"
        self.plan.parent.mkdir()
        self.plan.write_text(plan_text("review-plan-in-progress"), encoding="utf-8")
        self.review_root = self.root / "temp" / "review-plan" / "feat-x" / "example"

    def write_round(
        self,
        lane: str,
        round_number: int,
        *,
        ready: bool = True,
        snapshot: str | None = None,
    ) -> Path:
        round_dir = self.review_root / lane / f"round-{round_number:02d}"
        round_dir.mkdir(parents=True)
        previous = (
            "无。"
            if round_number == 1
            else "\n".join(
                [
                    "| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |",
                    "|---|------|------|----------|-------------|",
                    "| 1 | R1-1 | 缺少任务 | satisfactory | 实施步骤已补齐 |",
                    "",
                    "> 总结：R1-1 satisfactory。",
                ]
            )
        )
        findings = (
            "无。"
            if ready
            else "### 1. 缺少任务\n- **位置**：实施步骤\n- **观察**：缺少任务\n- **证据**：计划正文\n- **影响**：无法执行"
        )
        verdict = "可执行（Ready）" if ready else "需要完善（Needs Refinement）"
        review = round_dir / "review.md"
        review.write_text(
            "\n".join(
                [
                    f"# Review Round {round_number} — 计划审查",
                    "",
                    f"**计划**：{self.plan}",
                    f"**审查者**：{lane}",
                    "**姿态**：standard",
                    "**日期**：2026-09-16",
                    "",
                    "## 前轮问题核销",
                    "",
                    previous,
                    "",
                    "## 新问题与建议",
                    "",
                    findings,
                    "",
                    "## 同步清单（CONSISTENCY drift，非阻塞）",
                    "",
                    "无。",
                    "",
                    "## 本轮探索区域",
                    "",
                    "- Read 的源码文件：计划",
                    "",
                    "## 计划就绪状态",
                    "",
                    f"- **判定**：{verdict}",
                    "- **建议状态**：review-plan-complete",
                    "- **收敛趋势**：首轮",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        (round_dir / "plan-snapshot.md").write_text(
            snapshot if snapshot is not None else plan_text("review-plan-in-progress"),
            encoding="utf-8",
        )
        return review

    def run_finalize(self, *lanes: str, expected: str | None = None) -> tuple[int, str]:
        expected_hash = expected or finalizer.plan_hash(self.plan.read_text(encoding="utf-8"))
        argv = [
            "finalize",
            "--plan",
            str(self.plan),
            "--review-root",
            str(self.review_root),
            "--expected-plan-hash",
            expected_hash,
        ]
        for lane in lanes:
            argv += ["--lane", lane]
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            code = finalizer.main(argv)
        return code, stdout.getvalue()

    def assert_rejected(self, output: str, code: int, fragment: str) -> None:
        self.assertEqual(code, 1, output)
        self.assertTrue(output.startswith("FAIL\n"), output)
        self.assertIn(fragment, output)
        self.assertEqual(self.plan.read_text(encoding="utf-8"), plan_text("review-plan-in-progress"))

    def test_zero_finding_first_round_changes_only_the_status_field(self) -> None:
        reviews = [
            self.write_round(lane, 1, snapshot=plan_text("create-plan-complete"))
            for lane in ("claude", "codex", "gemini")
        ]
        before = [review.read_bytes() for review in reviews]
        code, output = self.run_finalize("claude:1", "codex:1", "gemini:1")
        self.assertEqual(code, 0, output)
        self.assertIn("FINALIZED=review-plan-complete", output)
        self.assertIn("LANES=claude:1,codex:1,gemini:1", output)
        self.assertEqual(self.plan.read_text(encoding="utf-8"), plan_text("review-plan-complete"))
        self.assertEqual([review.read_bytes() for review in reviews], before)

    def test_repaired_later_round_finalizes(self) -> None:
        self.write_round("claude", 1, ready=False)
        self.write_round("claude", 2)
        self.write_round("codex", 1)
        self.write_round("gemini", 1)
        code, output = self.run_finalize("claude:2", "codex:1", "gemini:1")
        self.assertEqual(code, 0, output)
        self.assertEqual(self.plan.read_text(encoding="utf-8"), plan_text("review-plan-complete"))

    def test_hash_command_ignores_status_value(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            self.assertEqual(finalizer.main(["hash", "--plan", str(self.plan)]), 0)
        self.assertEqual(
            stdout.getvalue().strip(),
            f"PLAN_HASH={finalizer.plan_hash(plan_text('create-plan-complete'))}",
        )

    def test_missing_lane_artifact_fails_closed(self) -> None:
        self.write_round("claude", 1)
        code, output = self.run_finalize("claude:1", "codex:1")
        self.assert_rejected(output, code, "codex:1: missing review.md or plan-snapshot.md")

    def test_finding_round_fails_closed(self) -> None:
        self.write_round("claude", 1, ready=False)
        code, output = self.run_finalize("claude:1")
        self.assert_rejected(output, code, "claude:1: round is not a finding-free Ready verdict")

    def test_stale_round_fails_when_a_later_round_exists(self) -> None:
        self.write_round("claude", 1)
        self.write_round("claude", 2)
        code, output = self.run_finalize("claude:1")
        self.assert_rejected(output, code, "claude:1: stale round; later completed rounds exist: round-02")

    def test_plan_changed_after_review_fails_closed(self) -> None:
        self.write_round("claude", 1, snapshot=plan_text("review-plan-in-progress", "旧计划内容"))
        code, output = self.run_finalize("claude:1")
        self.assert_rejected(output, code, "is stale against current")

    def test_stale_expected_hash_fails_closed(self) -> None:
        self.write_round("claude", 1)
        code, output = self.run_finalize("claude:1", expected="0" * 64)
        self.assert_rejected(output, code, "stale plan hash")

    def test_mixed_hashes_across_lanes_fail_closed(self) -> None:
        self.write_round("claude", 1)
        self.write_round("codex", 1, snapshot=plan_text("review-plan-in-progress", "另一版本"))
        code, output = self.run_finalize("claude:1", "codex:1")
        self.assert_rejected(output, code, "mixed plan hashes across lanes")

    def test_duplicate_lane_fails_closed(self) -> None:
        self.write_round("claude", 1)
        code, output = self.run_finalize("claude:1", "claude:1")
        self.assert_rejected(output, code, "duplicate lane: claude")

    def test_wrong_plan_status_fails_closed(self) -> None:
        self.write_round("claude", 1)
        self.plan.write_text(plan_text("create-plan-complete"), encoding="utf-8")
        code, output = self.run_finalize("claude:1")
        self.assertEqual(code, 1, output)
        self.assertIn("plan status must be review-plan-in-progress", output)
        self.assertEqual(self.plan.read_text(encoding="utf-8"), plan_text("create-plan-complete"))


if __name__ == "__main__":
    unittest.main()
