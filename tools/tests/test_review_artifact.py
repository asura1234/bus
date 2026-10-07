"""Tests for shared review artifact validation and deterministic chat rendering."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "cli_extensions" / "review_artifact.py"


def _previous_round(round_number: int) -> str:
    if round_number == 1:
        return "无。"
    return """| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R1-1 | 旧问题 | satisfactory | 已修复 |

> 总结：R1-1 satisfactory。"""


EMPTY_PREVIOUS_ROUND = """| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|

> 总结：前轮无待核销 finding。"""


def _plan_review(round_number: int = 2) -> str:
    return f"""# Review Round {round_number} — 计划审查

**计划**：docs/plans/demo.md
**审查者**：default
**姿态**：standard
**日期**：2026-07-23

## 前轮问题核销

{_previous_round(round_number)}

## 新问题与建议

### 1. PLAN_FINDING
- **位置**：计划位置
- **观察**：计划观察
- **证据**：计划证据
- **影响**：计划影响
- **可能的修复 / 选项**：计划修复

## 同步清单（CONSISTENCY drift，非阻塞）

- PLAN_SYNC

## 本轮探索区域

- PLAN_EXPLORATION_NOISE

## 计划就绪状态

- **判定**：需要完善（Needs Refinement）
- **建议状态**：review-plan-in-progress
- **收敛趋势**：R1 1 → R2 1
"""


def _pr_review(round_number: int = 2) -> str:
    return f"""# Review Round {round_number} — 代码审查

**分支**：feature/demo @ abc1234
**基线**：origin/master @ def5678
**计划（如有）**：docs/plans/demo.md
**锁定目标**：只修复 demo 行为。
**审查者**：default
**姿态**：standard
**日期**：2026-07-23

## 前轮问题核销

{_previous_round(round_number)}

## 新问题与建议

### 1. PR_FINDING
- **位置**：src/demo.py:1
- **维度**：4. 正确性
- **观察**：PR_OBSERVATION
- **证据**：PR_EVIDENCE
- **影响**：PR_IMPACT
- **可能的修复**：PR_FIX

```text
## 本轮探索区域
FENCED_HEADING_MUST_SURVIVE
```

## 同步清单（CONSISTENCY drift，非阻塞）

- PR_SYNC

## 本轮探索区域

- PR_EXPLORATION_NOISE

## 代码就绪状态

- **判定**：Needs Refinement
- **收敛趋势**：R1 1 → R2 1
"""


def _run_cli(review: Path, output: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(SCRIPT),
            "render-response",
            "--review-file",
            str(review),
            "--output",
            str(output),
        ],
        capture_output=True,
        text=True,
    )


def _write(path: Path, content: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    return path


class _TempDirTestCase(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def tmp_path(self, name: str) -> Path:
        path = self.root / name
        path.mkdir()
        return path


class DeterministicRenderingTests(_TempDirTestCase):
    def test_all_modes_preserve_developer_context_and_collapse_only_bookkeeping(self) -> None:
        cases = [
            (
                "plan",
                _plan_review(),
                "**计划**：docs/plans/demo.md",
                "PLAN_FINDING",
                "PLAN_SYNC",
                "- **判定**：需要完善（Needs Refinement）",
            ),
            (
                "pr",
                _pr_review(),
                "**计划（如有）**：docs/plans/demo.md",
                "PR_FINDING",
                "PR_SYNC",
                "- **判定**：Needs Refinement",
            ),
        ]
        for name, content, context, finding, sync, verdict in cases:
            with self.subTest(mode=name):
                review = _write(self.tmp_path(name) / "round-02" / "review.md", content)
                output = review.with_name("chat-response.md")

                result = _run_cli(review, output)

                self.assertEqual(result.returncode, 0, result.stderr)
                rendered = output.read_text()
                self.assertEqual(result.stdout, rendered)
                for expected in (
                    context,
                    finding,
                    sync,
                    verdict,
                    "> 总结：R1-1 satisfactory。",
                ):
                    self.assertIn(expected, rendered)
                self.assertNotIn("| R1-1 |", rendered)
                self.assertNotIn("旧问题", rendered)
                self.assertNotIn("EXPLORATION_NOISE", rendered)

    def test_pr_round_two_matches_exact_golden_response(self) -> None:
        review = _write(self.root / "round-02" / "review.md", _pr_review())
        output = review.with_name("chat-response.md")

        result = _run_cli(review, output)

        expected = """# Review Round 2 — 代码审查

**分支**：feature/demo @ abc1234
**基线**：origin/master @ def5678
**计划（如有）**：docs/plans/demo.md
**锁定目标**：只修复 demo 行为。
**审查者**：default
**姿态**：standard
**日期**：2026-07-23

## 前轮问题核销

> 总结：R1-1 satisfactory。

## 新问题与建议

### 1. PR_FINDING
- **位置**：src/demo.py:1
- **维度**：4. 正确性
- **观察**：PR_OBSERVATION
- **证据**：PR_EVIDENCE
- **影响**：PR_IMPACT
- **可能的修复**：PR_FIX

```text
## 本轮探索区域
FENCED_HEADING_MUST_SURVIVE
```

## 同步清单（CONSISTENCY drift，非阻塞）

- PR_SYNC

## 代码就绪状态

- **判定**：Needs Refinement
- **收敛趋势**：R1 1 → R2 1
"""
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, expected)
        self.assertEqual(output.read_text(), expected)

    def test_round_one_keeps_canonical_no_previous_findings(self) -> None:
        review = _write(self.root / "round-01" / "review.md", _plan_review(1))
        output = review.with_name("chat-response.md")

        result = _run_cli(review, output)

        self.assertEqual(result.returncode, 0, result.stderr)
        rendered = output.read_text()
        self.assertIn("## 前轮问题核销\n\n无。\n\n## 新问题与建议", rendered)
        self.assertNotIn("PLAN_EXPLORATION_NOISE", rendered)

    def test_round_two_empty_previous_ledger_is_valid_for_all_modes(self) -> None:
        for name, content in (("plan", _plan_review()), ("pr", _pr_review())):
            with self.subTest(mode=name):
                review = _write(
                    self.tmp_path(name) / "round-02" / "review.md",
                    content.replace(_previous_round(2), EMPTY_PREVIOUS_ROUND),
                )
                output = review.with_name("chat-response.md")

                result = _run_cli(review, output)

                self.assertEqual(result.returncode, 0, result.stderr)
                rendered = output.read_text()
                self.assertIn("> 总结：前轮无待核销 finding。", rendered)
                self.assertNotIn("| # | 来源 |", rendered)


class RenderFailureTests(_TempDirTestCase):
    def test_empty_previous_ledger_rejects_nonempty_summary(self) -> None:
        contradictory_ledger = EMPTY_PREVIOUS_ROUND.replace(
            "> 总结：前轮无待核销 finding。",
            "> 总结：R1-1 satisfactory。",
        )
        for name, content in (("plan", _plan_review()), ("pr", _pr_review())):
            with self.subTest(mode=name):
                review = _write(
                    self.tmp_path(name) / "round-02" / "review.md",
                    content.replace(_previous_round(2), contradictory_ledger),
                )
                output = review.with_name("chat-response.md")

                result = _run_cli(review, output)

                self.assertEqual(result.returncode, 2)
                self.assertIn("空前轮核销表", result.stderr)
                self.assertFalse(output.exists())

    def test_nonempty_previous_ledger_rejects_empty_summary(self) -> None:
        for name, content in (("plan", _plan_review()), ("pr", _pr_review())):
            with self.subTest(mode=name):
                review = _write(
                    self.tmp_path(name) / "round-02" / "review.md",
                    content.replace(
                        "> 总结：R1-1 satisfactory。",
                        "> 总结：前轮无待核销 finding。",
                    ),
                )
                output = review.with_name("chat-response.md")

                result = _run_cli(review, output)

                self.assertEqual(result.returncode, 2)
                self.assertIn("非空前轮核销表", result.stderr)
                self.assertFalse(output.exists())

    def test_invalid_or_truncated_artifact_fails_without_overwriting_output(self) -> None:
        cases = [
            (
                lambda text: text.replace(
                    "\n> 总结：R1-1 satisfactory。\n",
                    "\n",
                ),
                "总结",
            ),
            (
                lambda text: text.replace(
                    "> 总结：R1-1 satisfactory。",
                    "> 总结：R1-1 satisfactory。\n\n> 总结：重复。",
                ),
                "总结",
            ),
            (
                lambda text: text.replace("- **判定**：Needs Refinement", ""),
                "最终判定",
            ),
            (
                lambda text: text.replace(
                    "## 本轮探索区域\n\n- PR_EXPLORATION_NOISE\n\n",
                    "",
                ),
                "固定 section",
            ),
        ]
        for index, (mutation, match) in enumerate(cases):
            with self.subTest(case=index):
                review = _write(
                    self.tmp_path(f"case-{index}") / "round-02" / "review.md",
                    mutation(_pr_review()),
                )
                output = review.with_name("chat-response.md")
                output.write_text("KEEP\n")

                result = _run_cli(review, output)

                self.assertEqual(result.returncode, 2)
                self.assertIn(match, result.stderr)
                self.assertEqual(output.read_text(), "KEEP\n")

    def test_output_cannot_overwrite_review_artifact(self) -> None:
        review = _write(self.root / "round-02" / "review.md", _pr_review())

        result = _run_cli(review, review)

        self.assertEqual(result.returncode, 2)
        self.assertIn("不得覆盖", result.stderr)
        self.assertEqual(review.read_text(), _pr_review())

    def test_malformed_previous_ledger_fails_without_hiding_it(self) -> None:
        cases = [
            (
                "| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |",
                "| # | malformed |",
                "表头",
            ),
            (
                "|---|------|------|----------|-------------|",
                "|---|---|",
                "分隔行",
            ),
            (
                "| 1 | R1-1 | 旧问题 | satisfactory | 已修复 |",
                "| 1 | R1-1 | 旧问题 | unknown | 已修复 |",
                "状态",
            ),
            (
                "| 1 | R1-1 | 旧问题 | satisfactory | 已修复 |",
                "| 1 | R1-1 | 旧问题 | satisfactory |",
                "列数",
            ),
        ]
        for index, (old, new, match) in enumerate(cases):
            with self.subTest(case=index):
                review = _write(
                    self.tmp_path(f"case-{index}") / "round-02" / "review.md",
                    _pr_review().replace(old, new),
                )
                output = review.with_name("chat-response.md")

                result = _run_cli(review, output)

                self.assertEqual(result.returncode, 2)
                self.assertIn(match, result.stderr)
                self.assertFalse(output.exists())

    def test_escaped_pipe_in_previous_ledger_cell_is_not_a_column(self) -> None:
        review = _write(
            self.root / "round-02" / "review.md",
            _pr_review().replace("已修复 |", r"`plan\|pr` 已修复 |"),
        )
        output = review.with_name("chat-response.md")

        result = _run_cli(review, output)

        self.assertEqual(result.returncode, 0)
        self.assertIn("> 总结：R1-1 satisfactory。", output.read_text())


if __name__ == "__main__":
    unittest.main()
