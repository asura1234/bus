import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from pr_format_check import (  # noqa: E402
    check_body,
    check_title,
    parse_body_template,
    parse_title_types,
    run,
)
from pr_goal_context import (  # noqa: E402
    build_context,
    extract_plan_section,
    locked_context_paths,
    prepare_context,
)


TEMPLATE_PATH = Path(__file__).resolve().parents[2] / "references" / "pr-template.md"
BODY_TEMPLATE_PATH = (
    Path(__file__).resolve().parents[4] / ".github" / "pull_request_template.md"
)
GATE_EVIDENCE = "- [x] gate-and-fix — PASS, head `0123abc`, report `temp/gate-and-fix/b/round-1.md`"
E2E_EVIDENCE = "- [x] e2e-test — GREEN, head `0123abc`, report `temp/e2e-tui/runs/run/report.md`"


def valid_body() -> str:
    return "\n".join(
        [
            "## 摘要",
            "",
            "- **主要变更**：校验 PR 工作流和验证证据",
            "- **大小**：`M`",
            "",
            "## 目标",
            "",
            "统一工作流校验",
            "",
            "## 非目标",
            "",
            "无",
            "",
            "## 变更内容",
            "",
            "- 统一 PR 模板与校验器",
            "",
            "## 自测 / Agent 测",
            "",
            E2E_EVIDENCE,
            GATE_EVIDENCE,
            "- [x] `./run test cli`：通过",
            "",
            "## 截图 / GIF",
            "",
            "无 UI 变更",
            "",
            "## 其他说明",
            "",
            "无",
        ]
    )


def valid_draft_body() -> str:
    return (
        valid_body()
        .replace(
            f"{E2E_EVIDENCE}\n{GATE_EVIDENCE}\n- [x] `./run test cli`：通过",
            "- [ ] e2e-test — pending\n- [ ] gate-and-fix — pending",
        )
        .replace(
            "## 其他说明\n\n无",
            "## 其他说明\n\n门禁由 orchestrator 安排，本次未提供",
        )
    )


class PrFormatCheckTest(unittest.TestCase):
    def setUp(self) -> None:
        self.template_text = TEMPLATE_PATH.read_text(encoding="utf-8")
        self.title_types = parse_title_types(self.template_text)
        self.sections, self.placeholders, self.boxes = parse_body_template(
            BODY_TEMPLATE_PATH.read_text(encoding="utf-8")
        )

    def check(self, body: str, phase: str = "final") -> list[str]:
        return check_body(body, self.sections, self.placeholders, self.boxes, phase=phase)

    def test_template_declares_types_and_sections(self) -> None:
        self.assertIn("feat", self.title_types)
        self.assertIn("infra", self.title_types)
        self.assertEqual(
            self.sections,
            [
                "摘要",
                "目标",
                "非目标",
                "变更内容",
                "自测 / Agent 测",
                "截图 / GIF",
                "其他说明",
            ],
        )

    def test_valid_title_and_body_pass(self) -> None:
        self.assertEqual(check_title("[infra] 统一工作流校验", self.title_types), [])
        self.assertEqual(self.check(valid_body()), [])

    def test_draft_phase_allows_only_scoped_pending_items(self) -> None:
        body = valid_draft_body()
        self.assertEqual(self.check(body, phase="draft"), [])

        final_problems = self.check(body)
        self.assertTrue(any("unchecked boxes" in problem for problem in final_problems))

        draft_problems = self.check(valid_body(), phase="draft")
        self.assertTrue(any("needs at least one pending item" in problem for problem in draft_problems))

        malformed_pending_test = body.replace(
            "- [ ] e2e-test — pending\n- [ ] gate-and-fix — pending",
            "- [ ]e2e-test — pending\n- [ ]gate-and-fix — pending",
        )
        draft_problems = self.check(malformed_pending_test, phase="draft")
        self.assertTrue(any("needs at least one pending item" in problem for problem in draft_problems))

        half_proven = body.replace("- [ ] gate-and-fix — pending", GATE_EVIDENCE)
        self.assertEqual(self.check(half_proven, phase="draft"), [])

        unchecked_summary = body.replace(
            "- **主要变更**：校验 PR 工作流和验证证据",
            "- [ ] 摘要待补",
        )
        draft_problems = self.check(unchecked_summary, phase="draft")
        self.assertTrue(
            any("section `摘要` contains unchecked" in problem for problem in draft_problems)
        )

    def test_title_rejects_unknown_type_and_bad_shape(self) -> None:
        self.assertTrue(check_title("infra: 说明", self.title_types))
        self.assertTrue(check_title("[nope] 说明", self.title_types))
        self.assertTrue(check_title("[feat]", self.title_types))
        self.assertTrue(check_title(" [feat] 说明 ", self.title_types))

    def test_body_rejects_missing_or_reordered_sections(self) -> None:
        body = valid_body().replace("## 其他说明", "## 备注")
        problems = self.check(body)
        self.assertTrue(any("H2 sections" in problem for problem in problems))

    def test_body_rejects_missing_required_sections(self) -> None:
        body = valid_body().replace("## 截图 / GIF\n\n无 UI 变更\n\n", "")
        problems = self.check(body)
        self.assertTrue(any("H2 sections" in problem for problem in problems))

    def test_body_rejects_unchecked_boxes_and_empty_self_test(self) -> None:
        body = valid_body().replace(
            "- [x] `./run test cli`：通过", "- [ ] `./run test cli`：未跑"
        )
        problems = self.check(body)
        self.assertTrue(any("unchecked" in problem for problem in problems))

        body = valid_body().replace(
            f"{E2E_EVIDENCE}\n{GATE_EVIDENCE}\n- [x] `./run test cli`：通过", "手工确认过"
        )
        problems = self.check(body)
        self.assertTrue(any("at least one checked item" in problem for problem in problems))

    def test_template_self_test_boxes_are_required(self) -> None:
        self.assertEqual(self.boxes, ["e2e-test", "gate-and-fix"])

        problems = self.check(valid_body().replace(f"{GATE_EVIDENCE}\n", ""))
        self.assertTrue(any("exactly one `gate-and-fix` box" in problem for problem in problems))

        problems = self.check(
            valid_body().replace(GATE_EVIDENCE, f"{GATE_EVIDENCE}\n{GATE_EVIDENCE}")
        )
        self.assertTrue(any("exactly one `gate-and-fix` box" in problem for problem in problems))

    def test_checked_template_box_needs_head_and_report(self) -> None:
        for unproven in (
            "- [x] gate-and-fix",
            "- [x] gate-and-fix — PASS, report `temp/gate-and-fix/b/round-1.md`",
            "- [x] gate-and-fix — PASS, head `0123abc`",
        ):
            problems = self.check(valid_body().replace(GATE_EVIDENCE, unproven))
            self.assertTrue(
                any("checked `gate-and-fix` box must cite" in problem for problem in problems),
                unproven,
            )

    def test_writing_guidance_names_only_template_sections(self) -> None:
        """The guidance keys its rules by section name; a renamed template section must not leave it stale."""
        guidance = [
            line.removeprefix("### ")
            for line in self.template_text.split("# PR 描述", 1)[1].splitlines()
            if line.startswith("### ")
        ]
        self.assertEqual(guidance, self.sections)
        self.assertNotIn("\n## ", self.template_text.split("# PR 描述", 1)[1])

    def test_body_rejects_comments_and_invalid_size(self) -> None:
        problems = self.check(valid_body().replace("无 UI 变更", "<!-- 待补 -->"))
        self.assertTrue(any("HTML comments" in problem for problem in problems))

        problems = self.check(valid_body().replace("- **大小**：`M`", "- **大小**：`巨大`"))
        self.assertTrue(any("大小" in problem for problem in problems))

    def test_placeholders_are_derived_from_the_template(self) -> None:
        """Placeholders must come from the template itself, not from a list hard-coded in the script.

        A hard-coded list covers only the few thought of at the time: the template's goal slots and
        the change-list slot could both slip through, and a PR body that keeps them would still pass, while the
        whole point of the gate is to block exactly this residue.
        """
        self.assertIn("[按主题列出变更，不罗列文件名]", self.placeholders)
        self.assertIn("[这个 PR 要达成什么]", self.placeholders)
        self.assertIn("[刻意不做的事；没有写「无」]", self.placeholders)

        for placeholder in self.placeholders:
            problems = self.check(valid_body().replace("无 UI 变更", placeholder))
            self.assertTrue(
                any("placeholder left in place" in problem for problem in problems),
                f"未挡住模板占位符：{placeholder}",
            )

    def test_placeholder_inside_a_fence_is_not_residue(self) -> None:
        """A placeholder inside a fence is a **quotation**, not residue.

        Unchecked checkboxes and HTML comments are both judged "outside code fences"; if placeholders
        scanned the raw body, a PR that legitimately quotes a template fragment would be judged unfinished.
        """
        body = valid_body().replace(
            "无 UI 变更",
            "\n".join(["```markdown", "- [变更项 1]", "```", "见上"]),
        )
        self.assertEqual(self.check(body), [])

    def test_fenced_code_does_not_trigger_checks(self) -> None:
        body = valid_body().replace(
            "无 UI 变更",
            "\n".join(["```markdown", "- [ ] 这是代码示例", "<!-- 注释 -->", "```", "见上"]),
        )
        self.assertEqual(self.check(body), [])

    def test_run_validates_against_real_template(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            body_path = Path(directory) / "body.md"
            context_path = Path(directory) / "goal-context.md"
            locked_goal_path = Path(directory) / ".locked-goal"
            body_path.write_text(valid_body(), encoding="utf-8")
            context_path.write_text(
                "## 目标\n\n统一工作流校验\n\n## 非目标\n\n无\n",
                encoding="utf-8",
            )
            locked_goal_path.write_text("统一工作流校验\n", encoding="utf-8")
            self.assertEqual(
                run(
                    TEMPLATE_PATH,
                    "[infra] 统一工作流校验",
                    body_path,
                    goal_context_path=context_path,
                    locked_goal_path=locked_goal_path,
                ),
                [],
            )

            body_path.write_text(valid_draft_body(), encoding="utf-8")
            self.assertEqual(
                run(
                    TEMPLATE_PATH,
                    "[infra] 统一工作流校验",
                    body_path,
                    goal_context_path=context_path,
                    locked_goal_path=locked_goal_path,
                    phase="draft",
                ),
                [],
            )

    def test_plan_context_appends_verbatim_sections_in_argument_order(self) -> None:
        first = "\n".join(
            [
                "# A",
                "",
                "## 目标",
                "",
                "第一目标。  ",
                "- 保留 **Markdown**。",
                "",
                "## 非目标",
                "",
                "- 不做 A。",
                "",
                "## 实现",
                "",
                "无。",
                "",
            ]
        )
        second = """# B

## 目标

第二目标。

```text
## 围栏内不是 section
```

## 非目标

- 不做 B。
"""
        goal, non_goal = build_context([first, second])
        self.assertEqual(
            goal,
            "第一目标。  \n- 保留 **Markdown**。\n\n第二目标。\n\n```text\n## 围栏内不是 section\n```",
        )
        self.assertEqual(non_goal, "- 不做 A。\n\n- 不做 B。")

    def test_plan_context_rejects_missing_duplicate_or_empty_sections(self) -> None:
        with self.assertRaisesRegex(ValueError, "缺少.*非目标"):
            build_context(["## 目标\n\n有目标\n"])
        with self.assertRaisesRegex(ValueError, "重复.*目标"):
            build_context(["## 目标\n\n一\n\n## 目标\n\n二\n\n## 非目标\n\n无\n"])
        with self.assertRaisesRegex(ValueError, "目标.*为空"):
            extract_plan_section("## 目标\n\n## 非目标\n\n无\n", "目标")

    def test_prepare_context_writes_fragment_and_review_lock(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan_a = root / "a.md"
            plan_b = root / "b.md"
            output = root / "pr-goal-context.md"
            plan_a.write_text("## 目标\n\n目标 A\n\n## 非目标\n\n非目标 A\n", encoding="utf-8")
            plan_b.write_text("## 目标\n\n目标 B\n\n## 非目标\n\n非目标 B\n", encoding="utf-8")

            locked = prepare_context(
                branch="feat/a/b",
                output_path=output,
                repo_root=root,
                plan_paths=[plan_a, plan_b],
            )

            self.assertEqual(
                output.read_text(encoding="utf-8"),
                "## 目标\n\n目标 A\n\n目标 B\n\n## 非目标\n\n非目标 A\n\n非目标 B\n",
            )
            self.assertEqual(locked, root / "temp/review-pr/feat-a-b/.locked-goal")
            self.assertEqual(locked.read_text(encoding="utf-8"), "目标 A\n\n目标 B\n")
            self.assertEqual(
                (root / "temp/review-pr/feat-a-b/.locked-non-goals").read_text(encoding="utf-8"),
                "非目标 A\n\n非目标 B\n",
            )

    def test_prepare_context_uses_authored_goal_when_no_plan_exists(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            goal = root / "goal.md"
            non_goal = root / "non-goal.md"
            output = root / "pr-goal-context.md"
            goal.write_text("代理生成的目标\n", encoding="utf-8")
            non_goal.write_text("无\n", encoding="utf-8")

            locked = prepare_context(
                branch="infra/pr-skill",
                output_path=output,
                repo_root=root,
                goal_path=goal,
                non_goal_path=non_goal,
            )

            self.assertEqual(
                output.read_text(encoding="utf-8"),
                "## 目标\n\n代理生成的目标\n\n## 非目标\n\n无\n",
            )
            self.assertEqual(locked.read_text(encoding="utf-8"), "代理生成的目标\n")
            self.assertEqual(
                locked_context_paths(root, "infra/pr-skill")[1].read_text(encoding="utf-8"),
                "无\n",
            )

    def test_prepare_context_rejects_mixed_plan_and_authored_input(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan = root / "plan.md"
            plan.write_text("## 目标\n\n计划目标\n\n## 非目标\n\n计划非目标\n", encoding="utf-8")
            goal = root / "goal.md"
            non_goal = root / "non-goal.md"
            goal.write_text("目标\n", encoding="utf-8")
            non_goal.write_text("无\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "混用"):
                prepare_context(
                    branch="feat/room",
                    output_path=root / "pr-goal-context.md",
                    repo_root=root,
                    plan_paths=[plan],
                    goal_path=goal,
                    non_goal_path=non_goal,
                )
            self.assertFalse(locked_context_paths(root, "feat/room")[0].exists())

    def test_run_rejects_pr_or_lock_drift_from_prepared_context(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            body_path = root / "body.md"
            context_path = root / "goal-context.md"
            locked_goal_path = root / ".locked-goal"
            context_path.write_text(
                "## 目标\n\n计划原文\n\n## 非目标\n\n- 不做扩展\n",
                encoding="utf-8",
            )
            locked_goal_path.write_text("计划原文\n", encoding="utf-8")

            body_path.write_text(valid_body(), encoding="utf-8")
            problems = run(
                TEMPLATE_PATH,
                "[infra] 统一工作流校验",
                body_path,
                goal_context_path=context_path,
                locked_goal_path=locked_goal_path,
            )
            self.assertTrue(
                any("`目标`" in item and "prepared context" in item for item in problems)
            )
            self.assertTrue(
                any("`非目标`" in item and "prepared context" in item for item in problems)
            )

            body_path.write_text(
                valid_body().replace("统一工作流校验", "计划原文").replace("\n无\n", "\n- 不做扩展\n"),
                encoding="utf-8",
            )
            locked_goal_path.write_text("被改写的目标\n", encoding="utf-8")
            problems = run(
                TEMPLATE_PATH,
                "[infra] 统一工作流校验",
                body_path,
                goal_context_path=context_path,
                locked_goal_path=locked_goal_path,
            )
            self.assertTrue(any("locked goal" in item for item in problems))


if __name__ == "__main__":
    unittest.main()
