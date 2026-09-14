#!/usr/bin/env python3
"""execute-plan driver 的最小 transition bookkeeping 与 action envelope。"""

from __future__ import annotations

import hashlib
import json
from typing import Any, Iterable


SCHEMA_VERSION = 2
KIND = "execute-plan-driver-state"
ACTION_FIELDS = (
    "ACTION_ID",
    "EXPECTED_STATE_HASH",
    "ACTION",
    "TASK_ID",
    "GENERATION",
    "INPUT_ARTIFACTS",
    "EXPECTED_OUTPUT",
    "COMMAND",
)


class DriverError(ValueError):
    """driver transition 或 action identity 无效。"""


def _stable_json(value: Any) -> str:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )


def _sha256(value: Any) -> str:
    return hashlib.sha256(_stable_json(value).encode()).hexdigest()


def create_state(
    *,
    plan: str,
    tasks: Iterable[dict[str, Any]],
) -> dict[str, Any]:
    """创建只含 transition bookkeeping 的 driver state。"""

    task_states: dict[str, dict[str, Any]] = {}
    for task in tasks:
        task_id = int(task["id"])
        if task_id <= 0 or str(task_id) in task_states:
            raise DriverError("task id 必须是唯一正整数")
        blocked_by = tuple(int(value) for value in task.get("blocked_by", ()))
        task_states[str(task_id)] = {
            "name": str(task["name"]),
            "blocked_by": list(blocked_by),
            "consumes": list(int(value) for value in task.get("consumes", ())),
            "status": "READY" if task.get("done") else "PENDING",
            "generation": 0,
            "review_round": 0,
            "active_action": None,
            "latest_output": None,
            "accepted_artifact": None,
            "accepted_artifact_hash": None,
            "retained_delta": False,
            "reset_to_execution_base": False,
            "contract_hash": str(task.get("contract_hash", "")),
        }
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "plan": plan,
        "phase": "TASKS",
        "tasks": task_states,
    }


def validate_state(state: dict[str, Any]) -> None:
    actual_version = state.get("schema_version")
    if actual_version != SCHEMA_VERSION:
        raise DriverError(
            "driver state schema 不兼容："
            f"expected={SCHEMA_VERSION}, actual={actual_version}；"
            "请归档旧 temp/execute-plan state 后重新 start"
        )
    if state.get("kind") != KIND or not isinstance(state.get("tasks"), dict):
        raise DriverError("driver state kind/tasks 不合法")


def task_state_hash(state: dict[str, Any], task_id: int) -> str:
    """绑定单 task transition；其他 task 的并行推进不使 action 迟到。"""

    validate_state(state)
    task = state["tasks"].get(str(task_id))
    if not isinstance(task, dict):
        raise DriverError(f"task 不存在：{task_id}")
    identity = {
        "plan": state["plan"],
        "task_id": task_id,
        "name": task["name"],
        "status": task["status"],
        "blocked_by": task["blocked_by"],
        "consumes": task.get("consumes", []),
        "generation": task["generation"],
        "review_round": task["review_round"],
        "latest_output": task["latest_output"],
        "accepted_artifact": task.get("accepted_artifact"),
        "accepted_artifact_hash": task.get("accepted_artifact_hash"),
        "retained_delta": task.get("retained_delta", False),
        "reset_to_execution_base": task.get("reset_to_execution_base", False),
        "contract_hash": task.get("contract_hash", ""),
        "resume_context": task.get("resume_context"),
    }
    return _sha256(identity)


def _normalized_action(
    *,
    action: str,
    task_id: int,
    generation: int,
    input_artifacts: tuple[str, ...],
    expected_output: str,
    expected_state_hash: str,
    command: str,
) -> dict[str, str]:
    payload = {
        "EXPECTED_STATE_HASH": expected_state_hash,
        "ACTION": action,
        "TASK_ID": str(task_id),
        "GENERATION": str(generation),
        "INPUT_ARTIFACTS": ",".join(input_artifacts) if input_artifacts else "none",
        "EXPECTED_OUTPUT": expected_output or "none",
        "COMMAND": command or action,
    }
    return {"ACTION_ID": _sha256(payload), **payload}


def allocate_action(
    state: dict[str, Any],
    *,
    task_id: int,
    action: str,
    generation: int,
    input_artifacts: tuple[str, ...],
    expected_output: str,
    command: str = "",
) -> dict[str, str]:
    """幂等分配当前 task action；不覆盖未消费 action。"""

    task = state["tasks"].get(str(task_id))
    if not isinstance(task, dict):
        raise DriverError(f"task 不存在：{task_id}")
    existing = task.get("active_action")
    if existing is not None:
        candidate = _normalized_action(
            action=action,
            task_id=task_id,
            generation=generation,
            input_artifacts=input_artifacts,
            expected_output=expected_output,
            expected_state_hash=task_state_hash(state, task_id),
            command=command,
        )
        if existing != candidate:
            raise DriverError("task 已有未消费的当前 action")
        return existing
    if generation <= 0 or generation < int(task["generation"]):
        raise DriverError("generation 必须单调递增且不得复用")
    task["generation"] = generation
    expected_hash = task_state_hash(state, task_id)
    action_payload = _normalized_action(
        action=action,
        task_id=task_id,
        generation=generation,
        input_artifacts=input_artifacts,
        expected_output=expected_output,
        expected_state_hash=expected_hash,
        command=command,
    )
    task["active_action"] = action_payload
    return action_payload


def ingest_action(
    state: dict[str, Any],
    *,
    action_id: str,
    expected_state_hash: str,
    next_status: str,
    output_artifact: str,
) -> int:
    """只消费当前 action；旧 generation 或旧 action 不得推进状态。"""

    for raw_task_id, task in state["tasks"].items():
        active = task.get("active_action")
        if not isinstance(active, dict) or active.get("ACTION_ID") != action_id:
            continue
        task_id = int(raw_task_id)
        if active.get("EXPECTED_STATE_HASH") != expected_state_hash:
            raise DriverError("action expected state hash 与调用方不一致")
        if task_state_hash(state, task_id) != expected_state_hash:
            raise DriverError("stale action：当前 task state 已变化")
        task["status"] = next_status
        task["latest_output"] = output_artifact
        task["active_action"] = None
        task.pop("resume_context", None)
        return task_id
    raise DriverError("迟到 action 或 action 不是任何 task 的当前 action")


def render_action(action: dict[str, str]) -> str:
    """生成固定 action envelope。"""

    missing = [field for field in ACTION_FIELDS if not action.get(field)]
    if missing:
        raise DriverError("action 缺少字段：" + ", ".join(missing))
    return "EXECUTE_PLAN_ACTION\n" + "\n".join(
        f"{field}={action[field]}" for field in ACTION_FIELDS
    ) + "\n"
