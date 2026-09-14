#!/usr/bin/env python3
"""发布并校验 remediation 的 generation delta / cumulative task 文件集。"""

from __future__ import annotations

import hashlib
from pathlib import Path, PurePosixPath
from typing import Any

from attempt_manifest import capture, changed_paths
from attempt_manifest import load as load_manifest
from execute_plan_common import (
    ExecutePlanRuntimeError,
    attempt_before,
    load_json,
    relative,
)
from task_graph_selection import select_task_block
from verify_task_graph import parse_task_blocks
from workspace_manifest import atomic_create_json


SCHEMA_VERSION = 1
KIND = "execute-plan-task-file-scope"


def _owned_files(
    plan: Path,
    task_id: int,
    delta: tuple[str, ...],
) -> tuple[str, ...]:
    selected = select_task_block(
        parse_task_blocks(plan.read_text()),
        task_id=task_id,
    )
    owners = tuple(
        PurePosixPath(path.rstrip("/")) for path in selected.task.owned_files
    )
    return tuple(
        path
        for path in delta
        if any(
            PurePosixPath(path) == owner or owner in PurePosixPath(path).parents
            for owner in owners
        )
    )


def cumulative_task_files(
    plan: Path,
    root: Path,
    state: dict[str, Any],
    task_id: int,
    checkpoint: Path,
) -> tuple[str, ...]:
    attempt = int(state["attempt"])
    delta = changed_paths(
        load_manifest(attempt_before(root, attempt)),
        load_manifest(checkpoint),
    )
    return _owned_files(plan, task_id, delta)


def generation_delta_files(
    repo: Path,
    plan: Path,
    state: dict[str, Any],
    task_id: int,
    report_dir: Path,
) -> tuple[str, ...]:
    """返回当前 generation 相对派发时刻的 owner 内增量。"""

    attempt = int(state["attempt"])
    generation_before = report_dir / "generation-before.json"
    current = report_dir / "generation-after.json"
    capture(repo, plan, attempt, (task_id,), current)
    delta = changed_paths(
        load_manifest(generation_before),
        load_manifest(current),
    )
    return _owned_files(plan, task_id, delta)


def _files(values: object, field: str) -> tuple[str, ...]:
    if not isinstance(values, list) or not all(
        isinstance(value, str) for value in values
    ):
        raise ExecutePlanRuntimeError(f"{field} 必须是路径数组")
    normalized = tuple(sorted(values))
    if len(normalized) != len(set(normalized)):
        raise ExecutePlanRuntimeError(f"{field} 不得重复")
    for value in normalized:
        path = PurePosixPath(value)
        if (
            path.is_absolute()
            or not path.parts
            or any(part in {"", ".", ".."} for part in path.parts)
            or path.as_posix() != value
        ):
            raise ExecutePlanRuntimeError(f"{field} 含非规范路径：{value}")
    return normalized


def publish_task_file_scope(
    repo: Path,
    output: Path,
    *,
    task_id: int,
    generation: int,
    report: Path,
    generation_delta_files: tuple[str, ...],
    cumulative_task_files: tuple[str, ...],
) -> dict[str, Any]:
    delta = tuple(sorted(generation_delta_files))
    cumulative = tuple(sorted(cumulative_task_files))
    payload = {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "task_id": task_id,
        "generation": generation,
        "report": relative(repo, report),
        "report_hash": hashlib.sha256(report.read_bytes()).hexdigest(),
        "generation_delta_files": list(delta),
        "generation_delta_file_count": len(delta),
        "cumulative_task_files": list(cumulative),
        "cumulative_task_file_count": len(cumulative),
    }
    if not atomic_create_json(output, payload):
        if load_json(output) != payload:
            raise ExecutePlanRuntimeError("task file scope 已存在且内容冲突")
    return payload


def load_task_file_scope(
    repo: Path,
    path: Path,
    *,
    report: Path,
    task_id: int,
    generation: int,
) -> dict[str, Any]:
    payload = load_json(path)
    expected = {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "task_id": task_id,
        "generation": generation,
        "report": relative(repo, report),
        "report_hash": hashlib.sha256(report.read_bytes()).hexdigest(),
    }
    if any(payload.get(key) != value for key, value in expected.items()):
        raise ExecutePlanRuntimeError("task file scope identity/report hash 不匹配")
    delta = _files(
        payload.get("generation_delta_files"),
        "generation_delta_files",
    )
    cumulative = _files(
        payload.get("cumulative_task_files"),
        "cumulative_task_files",
    )
    if payload.get("generation_delta_file_count") != len(delta):
        raise ExecutePlanRuntimeError("generation delta count 不匹配")
    if payload.get("cumulative_task_file_count") != len(cumulative):
        raise ExecutePlanRuntimeError("cumulative task file count 不匹配")
    return {
        **payload,
        "generation_delta_files": list(delta),
        "cumulative_task_files": list(cumulative),
    }
