#!/usr/bin/env python3
"""把结构化 repair 收敛为 fresh generation + fresh active attempt。"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from attempt_manifest import ensure_only_plan_changed  # noqa: E402
from execute_plan_common import (  # noqa: E402
    ExecutePlanRuntimeError,
    load_json,
    relative,
    runtime_paths,
    save_state,
    task_state,
)
from execute_plan_contracts import task_contract_identities  # noqa: E402
from execute_plan_lifecycle import next_action, start_attempt  # noqa: E402
from verify_task_graph import parse_task_blocks  # noqa: E402


def _refresh_graph_state(plan: Path, state: dict[str, Any]) -> set[int]:
    plan_text = plan.read_text()
    _, contract_hashes = task_contract_identities(plan_text)
    parsed = parse_task_blocks(plan_text)
    tasks = {block.task.id: block.task for block in parsed}
    if set(tasks) != {int(task_id) for task_id in state["tasks"]}:
        raise ExecutePlanRuntimeError(
            "repair 不得增删 task identity；需要重新启动 execution"
        )
    changed: set[int] = set()
    for task_id, parsed_task in tasks.items():
        task = task_state(state, task_id)
        if task["name"] != parsed_task.name:
            raise ExecutePlanRuntimeError("repair 不得重命名 task identity")
        blocked_by = list(parsed_task.blocked_by)
        consumes = list(parsed_task.consumes)
        if task.get("contract_hash") != contract_hashes[task_id]:
            changed.add(task_id)
        task["blocked_by"] = blocked_by
        task["consumes"] = consumes
        task["contract_hash"] = contract_hashes[task_id]
    return changed


def _direct_consumers(state: dict[str, Any], producer_id: int) -> set[int]:
    return {
        int(task_id)
        for task_id, task in state["tasks"].items()
        if producer_id in task.get("consumes", [])
    }


def _mark_not_done(plan: Path, task_ids: set[int]) -> None:
    text = plan.read_text()
    for task_id in sorted(task_ids):
        block = next(
            block for block in parse_task_blocks(text) if block.task.id == task_id
        )
        changed, count = re.subn(
            r"(?m)^(\s*-\s*)\[[xX]\](\s*\*\*完成\*\*\s*)$",
            r"\1[ ]\2",
            block.text,
            count=1,
        )
        if count:
            text = text.replace(block.text, changed, 1)
    plan.write_text(text)


def _reopen_task(
    task: dict[str, Any],
    *,
    reset_to_execution_base: bool,
) -> None:
    task["status"] = "NEEDS_REFINEMENT"
    task["active_action"] = None
    task["review_round"] = 0
    task["accepted_artifact"] = None
    task["accepted_artifact_hash"] = None
    task["retained_delta"] = True
    task["reset_to_execution_base"] = reset_to_execution_base


def resume_repair(
    repo: Path,
    plan: Path,
    *,
    context: str | None,
) -> str:
    paths = runtime_paths(repo, plan)
    state = load_json(paths["state"])
    if state.get("phase") != "PLAN_REPAIR_REQUIRED":
        raise ExecutePlanRuntimeError("当前 phase 不是 PLAN_REPAIR_REQUIRED")
    repair = state.get("repair")
    if not isinstance(repair, dict):
        raise ExecutePlanRuntimeError("repair identity 缺失")
    checkpoint = repo / str(repair["checkpoint"])
    ensure_only_plan_changed(repo, plan, checkpoint)
    changed_edges = _refresh_graph_state(plan, state)
    task_id = int(repair["task_id"])
    reason = str(repair["reason"])
    affected = {task_id, *changed_edges}
    for changed_task_id in changed_edges:
        affected.update(_direct_consumers(state, changed_task_id))
    if reason == "upstream-contract":
        producer_id = repair.get("producer_task_id")
        if type(producer_id) is not int:
            raise ExecutePlanRuntimeError("upstream-contract 缺少 producer_task_id")
        blocked_task = task_state(state, task_id)
        if (
            producer_id not in blocked_task["blocked_by"]
            and producer_id not in blocked_task["consumes"]
        ):
            raise ExecutePlanRuntimeError("producer 不是当前 task 的直接依赖")
        affected.add(producer_id)
        affected.update(_direct_consumers(state, producer_id))
    elif reason in {"needs-context", "developer-decision"}:
        if not context:
            raise ExecutePlanRuntimeError(f"{reason} 必须提供 --context artifact")
        context_path = Path(context)
        if not context_path.is_absolute():
            context_path = repo / context_path
        if not context_path.is_file():
            raise ExecutePlanRuntimeError("repair context artifact 不存在")
        task_state(state, task_id)["resume_context"] = relative(repo, context_path)
    elif reason != "owner-graph-contract":
        raise ExecutePlanRuntimeError(f"未知 repair reason：{reason}")
    for raw_task_id, task in state["tasks"].items():
        if task.get("active_action") is not None:
            affected.add(int(raw_task_id))
    _mark_not_done(plan, affected)
    for affected_id in affected:
        _reopen_task(
            task_state(state, affected_id),
            reset_to_execution_base=True,
        )
    state["phase"] = "TASKS"
    state.pop("repair", None)
    start_attempt(
        repo,
        plan,
        paths,
        state,
        base=checkpoint,
    )
    save_state(paths["state"], state)
    return next_action(repo, plan, state["plan"], paths, state)
