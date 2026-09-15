import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))

from pr_format_check import check_body, check_title, parse_template, run  # noqa: E402
from pr_goal_context import (  # noqa: E402
    build_context,
    extract_plan_section,
    locked_context_paths,
    prepare_context,
)
from room_assignment_context import (  # noqa: E402
    OrchestratedContext,
    StandaloneContext,
    render_context as render_assignment_context,
)


TEMPLATE_PATH = Path(__file__).resolve().parents[1] / "references" / "pr-template.md"


def valid_body() -> str:
    return "\n".join(
        [
            "## 摘要",
            "",
            "- **主要变更**：重命名 update-docs 并合并两类文档审计",
            "- **大小**：`M`",
            "",
            "## 目标",
            "",
            "统一文档门禁",
            "",
            "## 非目标",
            "",
            "无",
            "",
            "## 变更内容",
            "",
            "- 重命名 skill 并扩展审计范围",
            "",
            "## 文档同步",
            "",
            "- [x] `shell/packages/example/AGENTS.md` — verified current",
            "",
            "## 自测 / Agent 测",
            "",
            "- [x] `./run test cli`：通过",
            "- [x] `./run lint check`：通过",
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
            "- [x] `shell/packages/example/AGENTS.md` — verified current",
            "- [ ] `update-docs`：待代码门禁完成后执行",
        )
        .replace(
            "- [x] `./run test cli`：通过\n- [x] `./run lint check`：通过",
            "- [ ] `gate-and-fix`：待执行",
        )
        .replace(
            "## 其他说明\n\n无",
            "## 其他说明\n\nDraft 已在 rebase 后创建；readiness convergence 待完成",
        )
    )


class PrFormatCheckTest(unittest.TestCase):
    def setUp(self) -> None:
        self.template_text = TEMPLATE_PATH.read_text(encoding="utf-8")
        self.title_types, self.sections, self.placeholders = parse_template(
            self.template_text
        )

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
                "文档同步",
                "自测 / Agent 测",
                "截图 / GIF",
                "其他说明",
            ],
        )

    def test_valid_title_and_body_pass(self) -> None:
        self.assertEqual(check_title("[infra] 统一模块文档门禁", self.title_types), [])
        self.assertEqual(check_body(valid_body(), self.sections, self.placeholders), [])

    def test_draft_phase_allows_only_scoped_pending_items(self) -> None:
        body = valid_draft_body()
        self.assertEqual(
            check_body(body, self.sections, self.placeholders, phase="draft"), []
        )

        final_problems = check_body(body, self.sections, self.placeholders)
        self.assertTrue(any("unchecked boxes" in problem for problem in final_problems))

        checked_docs = body.replace(
            "- [ ] `update-docs`：待代码门禁完成后执行",
            "- [x] `update-docs`：通过",
        )
        draft_problems = check_body(
            checked_docs, self.sections, self.placeholders, phase="draft"
        )
        self.assertTrue(any("draft phase" in problem for problem in draft_problems))

        no_pending_test = body.replace(
            "- [ ] `gate-and-fix`：待执行",
            "- [x] `./run test cli`：通过",
        )
        draft_problems = check_body(
            no_pending_test, self.sections, self.placeholders, phase="draft"
        )
        self.assertTrue(any("needs at least one pending item" in problem for problem in draft_problems))

        malformed_pending_test = body.replace(
            "- [ ] `gate-and-fix`：待执行",
            "- [ ]`gate-and-fix`：待执行",
        )
        draft_problems = check_body(
            malformed_pending_test, self.sections, self.placeholders, phase="draft"
        )
        self.assertTrue(any("needs at least one pending item" in problem for problem in draft_problems))

        unchecked_summary = body.replace(
            "- **主要变更**：重命名 update-docs 并合并两类文档审计",
            "- [ ] 摘要待补",
        )
        draft_problems = check_body(
            unchecked_summary, self.sections, self.placeholders, phase="draft"
        )
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
        problems = check_body(body, self.sections, self.placeholders)
        self.assertTrue(any("H2 sections" in problem for problem in problems))

    def test_body_rejects_unchecked_boxes_and_empty_self_test(self) -> None:
        body = valid_body().replace(
            "- [x] `./run test cli`：通过", "- [ ] `./run test cli`：未跑"
        )
        problems = check_body(body, self.sections, self.placeholders)
        self.assertTrue(any("unchecked" in problem for problem in problems))

        body = valid_body().replace("- [x] `./run test cli`：通过\n", "").replace(
            "- [x] `./run lint check`：通过", "手工确认过"
        )
        problems = check_body(body, self.sections, self.placeholders)
        self.assertTrue(any("at least one checked item" in problem for problem in problems))

    def test_body_rejects_placeholders_comments_and_hand_written_docs_sync(self) -> None:
        problems = check_body(
            valid_body().replace("无 UI 变更", "<!-- 待补 -->"),
            self.sections,
            self.placeholders,
        )
        self.assertTrue(any("HTML comments" in problem for problem in problems))

        problems = check_body(
            valid_body().replace(
                "- [x] `shell/packages/example/AGENTS.md` — verified current",
                "手写的架构说明",
            ),
            self.sections,
            self.placeholders,
        )
        self.assertTrue(any("renderer" in problem for problem in problems))

        problems = check_body(
            valid_body().replace("- **大小**：`M`", "- **大小**：`巨大`"),
            self.sections,
            self.placeholders,
        )
        self.assertTrue(any("大小" in problem for problem in problems))

    def test_placeholders_are_derived_from_the_template(self) -> None:
        """占位符必须来自模板本身，而不是脚本里写死的一份清单。

        写死的清单只覆盖当时想到的几个：模板里的目标槽位与 `[变更项 2]` 都可能
        漏网，留着它们的 PR 正文照样判通过——门禁的全部意义正是挡住这类残留。
        """
        self.assertIn("[变更项 2]", self.placeholders)
        self.assertIn("[目标原文或 agent 撰写的目标]", self.placeholders)
        self.assertIn("[非目标原文或无]", self.placeholders)
        # 标题小节的示例不是正文槽位：正文完全可能正当地讨论该用哪个类型。
        for title_example in ("[feat]", "[fix]", "[infra]", "[类型]"):
            self.assertNotIn(title_example, self.placeholders)

        for placeholder in self.placeholders:
            problems = check_body(
                valid_body().replace("无 UI 变更", placeholder),
                self.sections,
                self.placeholders,
            )
            self.assertTrue(
                any("placeholder left in place" in problem for problem in problems),
                f"未挡住模板占位符：{placeholder}",
            )

    def test_placeholder_inside_a_fence_is_not_residue(self) -> None:
        """围栏内的占位符是**引用**而不是残留。

        未勾选 checkbox 与 HTML 注释都按「代码块外」判定，占位符若改扫原始正文，一份
        正当引用了模板片段的 PR 会被判成没填完。
        """
        body = valid_body().replace(
            "无 UI 变更",
            "\n".join(["```markdown", "- [变更项 1]", "```", "见上"]),
        )
        self.assertEqual(check_body(body, self.sections, self.placeholders), [])

    def test_fenced_code_does_not_trigger_checks(self) -> None:
        body = valid_body().replace(
            "无 UI 变更",
            "\n".join(["```markdown", "- [ ] 这是代码示例", "<!-- 注释 -->", "```", "见上"]),
        )
        self.assertEqual(check_body(body, self.sections, self.placeholders), [])

    def test_run_validates_against_real_template(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            body_path = Path(directory) / "body.md"
            context_path = Path(directory) / "goal-context.md"
            locked_goal_path = Path(directory) / ".locked-goal"
            body_path.write_text(valid_body(), encoding="utf-8")
            context_path.write_text(
                "## 目标\n\n统一文档门禁\n\n## 非目标\n\n无\n",
                encoding="utf-8",
            )
            locked_goal_path.write_text("统一文档门禁\n", encoding="utf-8")
            self.assertEqual(
                run(
                    TEMPLATE_PATH,
                    "[infra] 统一模块文档门禁",
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
                    "[infra] 统一模块文档门禁",
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

    def write_assignment_context(self, root: Path, context) -> Path:
        path = root / "assignment-context.json"
        path.write_text(render_assignment_context(context), encoding="utf-8")
        return path

    def test_prepare_context_uses_verified_room_brief_when_planless(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            context = self.write_assignment_context(
                root,
                OrchestratedContext(
                    goal="房间目标\n- 逐字保留",
                    non_goals="不做 X",
                    assignment={"request_id": 4},
                ),
            )
            output = root / "pr-goal-context.md"

            locked = prepare_context(
                branch="feat/room",
                output_path=output,
                repo_root=root,
                assignment_context_path=context,
            )

            goal_lock, non_goals_lock = locked_context_paths(root, "feat/room")
            self.assertEqual(locked, goal_lock)
            self.assertEqual(
                output.read_text(encoding="utf-8"),
                "## 目标\n\n房间目标\n- 逐字保留\n\n## 非目标\n\n不做 X\n",
            )
            self.assertEqual(goal_lock.read_text(encoding="utf-8"), "房间目标\n- 逐字保留\n")
            self.assertEqual(non_goals_lock.read_text(encoding="utf-8"), "不做 X\n")

    def test_prepare_context_requires_plan_to_match_verified_room_brief(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan = root / "plan.md"
            plan.write_text("## 目标\n\n计划目标\n\n## 非目标\n\n计划非目标\n", encoding="utf-8")
            output = root / "pr-goal-context.md"
            matching = self.write_assignment_context(
                root, OrchestratedContext(goal="计划目标", non_goals="计划非目标")
            )
            prepare_context(
                branch="feat/room",
                output_path=output,
                repo_root=root,
                plan_paths=[plan],
                assignment_context_path=matching,
            )
            goal_lock, non_goals_lock = locked_context_paths(root, "feat/room")
            self.assertEqual(non_goals_lock.read_text(encoding="utf-8"), "计划非目标\n")

            goal_lock.unlink()
            non_goals_lock.unlink()
            drifted = self.write_assignment_context(
                root, OrchestratedContext(goal="计划目标", non_goals="房间另有非目标")
            )
            with self.assertRaisesRegex(ValueError, "不一致"):
                prepare_context(
                    branch="feat/room",
                    output_path=output,
                    repo_root=root,
                    plan_paths=[plan],
                    assignment_context_path=drifted,
                )
            self.assertFalse(goal_lock.exists())
            self.assertFalse(non_goals_lock.exists())

    def test_prepare_context_rejects_standalone_context_and_mixed_authored_input(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "pr-goal-context.md"
            standalone = self.write_assignment_context(root, StandaloneContext())
            with self.assertRaisesRegex(ValueError, "verified"):
                prepare_context(
                    branch="feat/room",
                    output_path=output,
                    repo_root=root,
                    assignment_context_path=standalone,
                )
            goal = root / "goal.md"
            non_goal = root / "non-goal.md"
            goal.write_text("目标\n", encoding="utf-8")
            non_goal.write_text("无\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "混用"):
                prepare_context(
                    branch="feat/room",
                    output_path=output,
                    repo_root=root,
                    goal_path=goal,
                    non_goal_path=non_goal,
                    assignment_context_path=standalone,
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
                "[infra] 统一模块文档门禁",
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
                valid_body().replace("统一文档门禁", "计划原文").replace("\n无\n", "\n- 不做扩展\n"),
                encoding="utf-8",
            )
            locked_goal_path.write_text("被改写的目标\n", encoding="utf-8")
            problems = run(
                TEMPLATE_PATH,
                "[infra] 统一模块文档门禁",
                body_path,
                goal_context_path=context_path,
                locked_goal_path=locked_goal_path,
            )
            self.assertTrue(any("locked goal" in item for item in problems))


if __name__ == "__main__":
    unittest.main()
