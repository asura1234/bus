#!/usr/bin/env python3
"""全局 EXIT CHECK 与 execute-plan finalizer 共用的命令语法。"""

from __future__ import annotations

import datetime
import re
import shlex
from dataclasses import dataclass

from task_graph_parser import (
    CREATED_DATE_LABEL_RE,
    CREATED_DATE_RE,
    _scan_section,
)


FINAL_GATE_REQUIRED_SINCE = "2026-07-24"
# The ordered source of truth for Bus final gates. The plan parser, order
# validator, and driver choices all derive from this tuple.
GATE_ORDER = (
    "lint",
    "unit",
    "build",
)
REQUIRED_GATE_PREFIX = GATE_ORDER[:2]
GATE_KINDS = frozenset(GATE_ORDER)
FULL_LINT_COMMAND = ("just", "lint")
CHANGED_LINT_PREFIX = ("just", "ci", "--", "--base")
EXECUTION_START_PLACEHOLDER = "<execution-start-ref>"
# Every kind is one exact repository command. Task-specific filters belong to
# TASK_LOCAL gates; final gates retain repository-wide meaning.
GATE_COMMANDS = {
    "unit": ("just", "test"),
}
BUILD_PREFIX = ("just", "build")
_COMMAND_RE = re.compile(r"`([^`\n]+)`")
# 有序或无序列表项才是 gate 声明；blockquote 与普通段落都是注解。
_DECLARATION_RE = re.compile(r"^\s*(?:\d+[.)]|[-*+])\s")
_EXIT_HEADING_RE = re.compile(r"^\s*###\s+全局 EXIT CHECK\s*$")


class FinalGateContractError(ValueError):
    """全局 EXIT CHECK 命令不符合 canonical 合同。"""


@dataclass(frozen=True)
class DeclaredFinalGate:
    kind: str
    command: tuple[str, ...]


def _requires_contract(plan_text: str) -> bool:
    if not CREATED_DATE_LABEL_RE.search(plan_text):
        return False
    match = CREATED_DATE_RE.search(plan_text)
    if match is None:
        return True
    try:
        created = datetime.date.fromisoformat(match.group(1))
    except ValueError:
        return True
    return created >= datetime.date.fromisoformat(FINAL_GATE_REQUIRED_SINCE)


def validate_command_shape(
    gate_kind: str,
    command: tuple[str, ...],
    *,
    declaration: bool,
) -> str | None:
    """校验 final gate argv；返回 changed-lint 的 base token。"""

    if gate_kind not in GATE_KINDS or not command:
        raise FinalGateContractError("final gate kind/command 不合法")
    if gate_kind == "lint":
        if command == FULL_LINT_COMMAND:
            return None
        if (
            command[: len(CHANGED_LINT_PREFIX)] != CHANGED_LINT_PREFIX
            or len(command) != len(CHANGED_LINT_PREFIX) + 1
        ):
            raise FinalGateContractError(
                "final lint 必须精确运行："
                f"{shlex.join(FULL_LINT_COMMAND)} 或 "
                f"{shlex.join(CHANGED_LINT_PREFIX)} {EXECUTION_START_PLACEHOLDER}"
            )
        base = command[-1]
        if declaration and base != EXECUTION_START_PLACEHOLDER:
            raise FinalGateContractError(
                "计划中的 final lint changed base 必须写 "
                f"{EXECUTION_START_PLACEHOLDER}"
            )
        return base
    expected = GATE_COMMANDS.get(gate_kind)
    if expected is not None:
        if command != expected:
            raise FinalGateContractError(
                f"final {gate_kind} 必须精确运行：{shlex.join(expected)}"
            )
        return None
    if command != BUILD_PREFIX:
        raise FinalGateContractError(
            f"final build must run exactly: {shlex.join(BUILD_PREFIX)}"
        )
    return None


def _kind(command: tuple[str, ...]) -> str:
    if command[:2] == ("just", "lint") or command[:2] == ("just", "ci"):
        return "lint"
    if command == GATE_COMMANDS["unit"]:
        return "unit"
    if command == BUILD_PREFIX:
        return "build"
    raise FinalGateContractError(
        "全局 EXIT CHECK 含非 canonical 命令：" + shlex.join(command)
    )


def declared_final_gates(plan_text: str) -> tuple[DeclaredFinalGate, ...]:
    """解析并校验测试计划中的全局 EXIT CHECK。"""

    lines, found_test_plan, scan_errors = _scan_section(plan_text, "测试计划")
    if scan_errors:
        raise FinalGateContractError("；".join(scan_errors))
    in_exit = False
    found_exit = False
    command_texts: list[str] = []
    for line in lines:
        if line.fenced:
            continue
        if _EXIT_HEADING_RE.fullmatch(line.text):
            in_exit = True
            found_exit = True
            continue
        if in_exit and re.match(r"^\s*###\s+", line.text):
            break
        if in_exit:
            # 只有列表项是声明。其余都是注解：模板自带的 `> **重要**` 块同时含两种
            # lint 形态与全部可选槽位示例，普通段落也常引用命令解释某个槽位为何缺席；
            # 把它们一并当成声明，逐字复制模板的计划反而必然报「lint gate 重复」。
            if not _DECLARATION_RE.match(line.text):
                continue
            command_texts.extend(_COMMAND_RE.findall(line.text))
    if not found_test_plan or not found_exit:
        if _requires_contract(plan_text):
            raise FinalGateContractError(
                "新计划缺少『测试计划 → 全局 EXIT CHECK』"
            )
        return ()
    if not command_texts:
        raise FinalGateContractError("全局 EXIT CHECK 缺少反引号精确命令")

    gates: list[DeclaredFinalGate] = []
    seen: set[str] = set()
    for command_text in command_texts:
        try:
            command = tuple(shlex.split(command_text))
        except ValueError as error:
            raise FinalGateContractError(
                f"全局 EXIT CHECK 命令无法解析：{command_text}"
            ) from error
        kind = _kind(command)
        validate_command_shape(kind, command, declaration=True)
        if kind in seen:
            raise FinalGateContractError(
                f"全局 EXIT CHECK 的 {kind} gate 重复"
            )
        seen.add(kind)
        gates.append(DeclaredFinalGate(kind, command))

    kinds = [gate.kind for gate in gates]
    expected = [kind for kind in GATE_ORDER if kind in seen]
    required = list(REQUIRED_GATE_PREFIX)
    if kinds != expected or kinds[: len(required)] != required:
        readable = " → ".join(
            kind if kind in REQUIRED_GATE_PREFIX else f"{kind}（可选）"
            for kind in GATE_ORDER
        )
        raise FinalGateContractError("全局 EXIT CHECK 顺序必须是 " + readable)
    return tuple(gates)


def validate_declared_execution(
    declared: DeclaredFinalGate,
    command: tuple[str, ...],
) -> None:
    """校验运行命令没有省略或替换计划声明的 final gate。"""

    runtime_base = validate_command_shape(
        declared.kind,
        command,
        declaration=False,
    )
    if declared.kind == "lint":
        declared_base = validate_command_shape(
            declared.kind,
            declared.command,
            declaration=True,
        )
        matches = (declared_base is None) == (runtime_base is None)
    else:
        matches = command == declared.command
    if not matches:
        raise FinalGateContractError(
            f"final {declared.kind} 命令与计划声明不一致"
        )


def check_final_gate_contract(plan_text: str) -> list[str]:
    try:
        declared_final_gates(plan_text)
    except FinalGateContractError as error:
        return [str(error)]
    return []
