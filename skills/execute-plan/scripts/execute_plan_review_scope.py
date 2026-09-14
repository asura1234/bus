#!/usr/bin/env python3
"""重算并验证 execute-plan 内部 task review artifact 的当前 scope identity。"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from execute_plan_common import (  # noqa: E402
    ExecutePlanRuntimeError,
    load_json,
)
from execute_plan_contracts import validated_plan_tasks  # noqa: E402
from task_agent_report import parse_report  # noqa: E402
from task_file_scope import load_task_file_scope  # noqa: E402
from task_graph_selection import select_task_block  # noqa: E402
from task_review_evidence import validate_task_evidence  # noqa: E402
from task_scope_evidence import canonical_context, scope_hashes  # noqa: E402
from verify_task_graph import (  # noqa: E402
    file_contract_for_owners,
    parse_task_blocks,
)


def validate_task_target(
    artifact_target: dict[str, str] | Any,
    *,
    plan_relative: str,
    task_id: int,
    task_name: str,
) -> None:
    if artifact_target.get("plan") != plan_relative:
        raise ExecutePlanRuntimeError(
            "review artifact plan 与当前 action 不一致"
        )
    expected = f"任务{task_id}：{task_name}"
    if artifact_target.get("task") != expected:
        raise ExecutePlanRuntimeError(
            f"review artifact task 不一致：expected={expected}"
        )


def current_review_scope_inputs(
    repo: Path,
    action: dict[str, Any],
    *,
    task_id: int,
    task_name: str,
    plan_relative: str,
) -> dict[str, Any]:
    action_inputs = action["INPUT_ARTIFACTS"].split(",")
    if len(action_inputs) < 3 or not action_inputs[2].endswith(
        "/file-scope.json"
    ):
        raise ExecutePlanRuntimeError(
            "REVIEW_TASK action 缺少 canonical file-scope"
        )
    report_relative = action_inputs[1]
    report_path = repo / report_relative
    report = parse_report(report_path.read_text())
    file_scope_relative = action_inputs[2]
    file_scope = load_task_file_scope(
        repo,
        repo / file_scope_relative,
        report=report_path,
        task_id=task_id,
        generation=report.generation,
    )
    generation_files = tuple(file_scope["generation_delta_files"])
    files = list(file_scope["cumulative_task_files"])
    if files != sorted(action_inputs[3:]):
        raise ExecutePlanRuntimeError(
            "review action cumulative files 与 file scope 不一致"
        )
    plan_text = (repo / plan_relative).read_text()
    validated_plan_tasks(plan_text)
    parsed_blocks = parse_task_blocks(plan_text)
    selected = select_task_block(parsed_blocks, task_id=task_id)
    evidence = validate_task_evidence(
        repo=repo,
        report_path=report_path,
        task_id=task_id,
        task_name=task_name,
        generation_files=generation_files,
    )
    producer_blocks = {block.task.id: block.text for block in parsed_blocks}
    context_hash, scope_hash = scope_hashes(
        context=canonical_context(plan_text),
        owner_file_contract=file_contract_for_owners(
            plan_text,
            selected.task.owned_files,
        ),
        task_block=selected.text,
        producer_blocks=tuple(
            producer_blocks[producer_id]
            for producer_id in selected.task.consumes
        ),
        files=[(repo / path, path) for path in files],
        evidence_hash=evidence.evidence_hash,
    )
    return {
        "plan": plan_relative,
        "task_id": task_id,
        "task_name": task_name,
        "report": report_relative,
        "file_scope": file_scope_relative,
        "files": sorted(files),
        "report_hash": evidence.evidence_hash,
        "plan_context_hash": context_hash,
        "scope_hash": scope_hash,
    }


def validate_review_scope_inputs(
    repo: Path,
    review: Path,
    action: dict[str, Any],
    artifact_scope_hash: str,
    *,
    task_id: int,
    task_name: str,
    plan_relative: str,
) -> None:
    inputs_path = review.parent / "scope-inputs.json"
    inputs = load_json(inputs_path)
    expected = current_review_scope_inputs(
        repo,
        action,
        task_id=task_id,
        task_name=task_name,
        plan_relative=plan_relative,
    )
    actual = {
        "plan": inputs.get("plan"),
        "task_id": inputs.get("task_id"),
        "task_name": inputs.get("task_name"),
        "report": inputs.get("report"),
        "file_scope": inputs.get("file_scope"),
        "files": sorted(inputs.get("files", []))
        if isinstance(inputs.get("files"), list)
        else None,
        "report_hash": inputs.get("report_hash"),
        "plan_context_hash": inputs.get("plan_context_hash"),
        "scope_hash": inputs.get("scope_hash"),
    }
    if actual != expected or artifact_scope_hash != expected["scope_hash"]:
        raise ExecutePlanRuntimeError(
            "review artifact scope 与当前 plan/report/files 内容不一致"
        )
    try:
        inputs_path.resolve().relative_to(repo)
    except ValueError as error:
        raise ExecutePlanRuntimeError(
            "review scope inputs 越出 repo"
        ) from error
