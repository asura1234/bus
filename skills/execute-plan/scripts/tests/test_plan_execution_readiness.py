from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

import plan_execution_gate  # noqa: E402
from task_graph_parser import Task  # noqa: E402
from task_graph_validation import (  # noqa: E402
    check_plan_path_closure,
    file_contract_for_owners,
)


class PlanExecutionReadinessTest(unittest.TestCase):
    def test_status_is_the_only_review_readiness_signal(self) -> None:
        plan = """\
**状态**：review-plan-complete
**当前计划完整程度**：1%

## 需要决策的事项

- **仍保留的旧格式决策**：状态已经是评审结论，因此执行器不重复推导评审结果。

## 需要修改/添加的文件

- [ ] **修改文件**：`src/example.ts`
"""
        fake_template = Path(__file__)
        with (
            patch.object(plan_execution_gate, "detect_template", return_value=(None, fake_template)),
            patch.object(plan_execution_gate, "check_template_match", return_value=[]),
            patch.object(plan_execution_gate, "check_goal_required", return_value=None),
            patch.object(plan_execution_gate, "check_goal_section", return_value=None),
            patch.object(plan_execution_gate, "check_section_has_content", return_value=None),
            patch.object(plan_execution_gate, "check_task_fields", return_value=[]),
            patch.object(plan_execution_gate, "parse_tasks", return_value=[]),
            patch.object(plan_execution_gate, "verify", return_value=[]),
            patch.object(plan_execution_gate, "check_plan_path_closure", return_value=[]),
            patch.object(plan_execution_gate, "check_gate_execution_levels", return_value=[]),
            patch.object(plan_execution_gate, "check_final_gate_contract", return_value=[]),
            patch.object(plan_execution_gate, "check_size_token", return_value=None),
            patch.object(plan_execution_gate, "check_loc_estimates", return_value=[]),
        ):
            self.assertEqual(plan_execution_gate.gate(plan), [])

    def test_file_contract_accepts_plain_and_legacy_bullets(self) -> None:
        plan = """\
**创建日期**：2026-09-10

## 需要修改/添加的文件

- **修改文件**：`src/existing.ts`
  - **用途**：canonical plain entry
- [ ] **新文件**：`src/new.ts`
  - **用途**：legacy unchecked entry

## 测试计划
"""
        task = Task(
            id=1,
            name="实现",
            owned_files=("src",),
            blocked_by=(),
            consumes=(),
            done=False,
        )

        self.assertEqual(check_plan_path_closure(plan, [task]), [])
        owner_contract = file_contract_for_owners(plan, task.owned_files)
        self.assertIn("- **修改文件**：`src/existing.ts`", owner_contract)
        self.assertIn("- [ ] **新文件**：`src/new.ts`", owner_contract)


if __name__ == "__main__":
    unittest.main()
