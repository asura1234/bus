from __future__ import annotations

import sys
import unittest
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from final_gate_contract import (  # noqa: E402
    GATE_ORDER,
    DeclaredFinalGate,
    FinalGateContractError,
    declared_final_gates,
    validate_command_shape,
    validate_declared_execution,
)


class FinalGateContractTest(unittest.TestCase):
    def test_accepts_bus_gates_in_order(self) -> None:
        plan = """\
**创建日期**：2026-09-14

## 测试计划

### 全局 EXIT CHECK

1. `just lint`
2. `just test`
3. `just build`
"""
        self.assertEqual(
            [gate.kind for gate in declared_final_gates(plan)],
            ["lint", "unit", "build"],
        )

    def test_requires_lint_and_test(self) -> None:
        plan = """\
**创建日期**：2026-09-14

## 测试计划

### 全局 EXIT CHECK

1. `just lint`
"""
        with self.assertRaisesRegex(FinalGateContractError, "unit"):
            declared_final_gates(plan)

    def test_rejects_noncanonical_commands(self) -> None:
        for kind, command in (
            ("lint", ("cargo", "clippy")),
            ("unit", ("cargo", "test")),
            ("build", ("cargo", "build")),
            ("build", ("just", "build", "release")),
        ):
            with self.subTest(kind=kind, command=command):
                with self.assertRaises(FinalGateContractError):
                    validate_command_shape(kind, command, declaration=True)

    def test_canonical_template_is_a_valid_declaration(self) -> None:
        template = (
            Path(__file__).resolve().parents[4]
            / "docs"
            / "templates"
            / "plan-template.md"
        )
        self.assertEqual(
            [gate.kind for gate in declared_final_gates(template.read_text())],
            ["lint", "unit", "build"],
        )

    def test_ignores_commands_in_commentary(self) -> None:
        plan = """\
**创建日期**：2026-09-14

## 测试计划

### 全局 EXIT CHECK

1. `just lint`
2. `just test`

> `just build` is optional.
"""
        self.assertEqual(
            [gate.kind for gate in declared_final_gates(plan)],
            ["lint", "unit"],
        )

    def test_run_final_choices_track_the_contract(self) -> None:
        import execute_plan

        parser = execute_plan._parser()
        run_final = next(
            action
            for action in parser._subparsers._group_actions[0].choices.values()
            if any(
                option == "--kind"
                for parser_action in action._actions
                for option in parser_action.option_strings
            )
        )
        kind_action = next(
            action for action in run_final._actions if "--kind" in action.option_strings
        )
        self.assertEqual(tuple(kind_action.choices), GATE_ORDER)

    def test_finalizer_accepts_declared_commands_verbatim(self) -> None:
        for kind, command in (
            ("lint", ("just", "lint")),
            ("unit", ("just", "test")),
            ("build", ("just", "build")),
        ):
            with self.subTest(kind=kind):
                validate_declared_execution(DeclaredFinalGate(kind, command), command)

    def test_finalizer_rejects_a_substituted_command(self) -> None:
        with self.assertRaisesRegex(FinalGateContractError, "just build"):
            validate_declared_execution(
                DeclaredFinalGate("build", ("just", "build")),
                ("cargo", "build", "--release"),
            )


if __name__ == "__main__":
    unittest.main()
