#!/usr/bin/env python3
"""捕获并验证当前 active task attempt 的共享工作树 owner-union 证据。"""

from __future__ import annotations

import hashlib
import json
import os
import re
from pathlib import Path
from typing import Any, Iterable

from execute_plan_contracts import validated_plan_tasks
from task_graph_validation import _owner_covers_path
from verify_task_graph import parse_task_blocks
from workspace_manifest import (
    atomic_create_json,
    canonical_repo_root,
    capture_entries,
    entry_to_dict,
    head,
)


SCHEMA_VERSION = 1
KIND = "execute-plan-attempt-manifest"


class AttemptManifestError(ValueError):
    """attempt manifest identity、contract 或 owner-union 不合法。"""


def _lexical(path: Path) -> Path:
    return Path(os.path.abspath(path.expanduser()))


def _plan_identity(repo: Path, raw_plan: Path) -> tuple[Path, str]:
    plan = _lexical(raw_plan if raw_plan.is_absolute() else repo / raw_plan)
    if not plan.is_file():
        raise AttemptManifestError(f"计划文件不存在：{plan}")
    try:
        return plan, plan.relative_to(repo).as_posix()
    except ValueError as error:
        raise AttemptManifestError("计划文件必须位于 repo 内") from error


def _validated_blocks(plan: Path) -> dict[int, Any]:
    plan_text = plan.read_text()
    try:
        validated_plan_tasks(plan_text)
        blocks = parse_task_blocks(plan_text)
    except ValueError as error:
        raise AttemptManifestError(str(error)) from error
    return {block.task.id: block for block in blocks}


def _normalized_block(text: str) -> str:
    return re.sub(
        r"(?m)^(\s*-\s*)\[[ xX]\](\s*\*\*完成\*\*\s*)$",
        r"\1[ ]\2",
        text,
        count=1,
    )


def _contract(plan: Path, task_ids: Iterable[int]) -> dict[str, Any]:
    selected_ids = sorted(set(int(task_id) for task_id in task_ids))
    if not selected_ids or selected_ids[0] <= 0:
        raise AttemptManifestError("attempt task_ids 必须是非空正整数集合")
    blocks = _validated_blocks(plan)
    missing = [task_id for task_id in selected_ids if task_id not in blocks]
    if missing:
        raise AttemptManifestError(
            "计划缺少 attempt task：" + ", ".join(map(str, missing))
        )
    selected = [blocks[task_id] for task_id in selected_ids]
    tasks = [
        {
            "id": block.task.id,
            "name": block.task.name,
            "owned_files": list(block.task.owned_files),
            "blocked_by": list(block.task.blocked_by),
            "consumes": list(block.task.consumes),
            "block": _normalized_block(block.text),
        }
        for block in selected
    ]
    encoded = json.dumps(
        tasks,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    return {
        "task_ids": selected_ids,
        "tasks": tasks,
        "contract_hash": hashlib.sha256(encoded.encode()).hexdigest(),
        "owner_paths": sorted(
            {
                path
                for block in selected
                for path in block.task.owned_files
            }
        ),
    }


def _output(repo: Path, plan: Path, raw_output: Path) -> Path:
    output = _lexical(raw_output if raw_output.is_absolute() else repo / raw_output)
    root = _lexical(repo / "temp" / "execute-plan" / plan.stem)
    try:
        output.relative_to(root)
    except ValueError as error:
        raise AttemptManifestError(
            f"attempt manifest 必须位于 canonical state root：{root}"
        ) from error
    if output.is_symlink():
        raise AttemptManifestError("attempt manifest output 不得是 symlink")
    return output


def _payload(
    repo: Path,
    plan: Path,
    plan_relative: str,
    attempt: int,
    task_ids: Iterable[int],
    entries: list[dict[str, Any]],
) -> dict[str, Any]:
    if attempt <= 0:
        raise AttemptManifestError("attempt 必须是正整数")
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "repo_root": str(repo),
        "plan": plan_relative,
        "attempt": attempt,
        "head": head(repo),
        **_contract(plan, task_ids),
        "entries": entries,
    }


def capture(
    repo_raw: Path,
    plan_raw: Path,
    attempt: int,
    task_ids: Iterable[int],
    output_raw: Path,
) -> dict[str, Any]:
    repo = canonical_repo_root(repo_raw)
    plan, plan_relative = _plan_identity(repo, plan_raw)
    output = _output(repo, plan, output_raw)
    payload = _payload(
        repo,
        plan,
        plan_relative,
        attempt,
        task_ids,
        [entry_to_dict(entry) for entry in capture_entries(repo)],
    )
    if atomic_create_json(output, payload):
        return payload
    existing = load(output)
    if existing != payload:
        raise AttemptManifestError(
            "attempt manifest 已存在且证据不同；必须使用新 attempt/checkpoint"
        )
    return existing


def derive_before(
    repo_raw: Path,
    plan_raw: Path,
    attempt: int,
    task_ids: Iterable[int],
    output_raw: Path,
    *,
    base_path: Path,
    accept_owner_paths: Iterable[str] = (),
    reset_path: Path | None = None,
    reset_owner_paths: Iterable[str] = (),
) -> dict[str, Any]:
    """从已验证 base 派生新 attempt；只接纳明确 owner，或把 owner 重置到 execution base。"""

    repo = canonical_repo_root(repo_raw)
    plan, plan_relative = _plan_identity(repo, plan_raw)
    output = _output(repo, plan, output_raw)
    base = load(_lexical(base_path))
    current = {
        entry.path: entry_to_dict(entry) for entry in capture_entries(repo)
    }
    entries = _entry_map(base)

    def replace_owners(source: dict[str, dict[str, Any]], owners: Iterable[str]) -> None:
        owner_list = tuple(owners)
        if not owner_list:
            return
        for path in set(entries) | set(source):
            if not any(_owner_covers_path(owner, path) for owner in owner_list):
                continue
            if path in source:
                entries[path] = source[path]
            else:
                entries.pop(path, None)

    replace_owners(current, accept_owner_paths)
    if reset_path is not None:
        replace_owners(_entry_map(load(_lexical(reset_path))), reset_owner_paths)
    if plan_relative in current:
        entries[plan_relative] = current[plan_relative]
    else:
        entries.pop(plan_relative, None)
    payload = _payload(
        repo,
        plan,
        plan_relative,
        attempt,
        task_ids,
        [entries[path] for path in sorted(entries)],
    )
    if not atomic_create_json(output, payload):
        existing = load(output)
        if existing != payload:
            raise AttemptManifestError("新 attempt before 已存在且证据不同")
        return existing
    return payload


def load(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise AttemptManifestError(f"无法读取 attempt manifest：{path}") from error
    if (
        not isinstance(payload, dict)
        or payload.get("schema_version") != SCHEMA_VERSION
        or payload.get("kind") != KIND
    ):
        raise AttemptManifestError("attempt manifest schema/kind 不合法")
    return payload


def _entry_map(payload: dict[str, Any]) -> dict[str, dict[str, Any]]:
    entries = payload.get("entries")
    if not isinstance(entries, list):
        raise AttemptManifestError("attempt manifest entries 不合法")
    result: dict[str, dict[str, Any]] = {}
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
            raise AttemptManifestError("attempt manifest entry 不合法")
        if entry["path"] in result:
            raise AttemptManifestError(f"manifest 路径重复：{entry['path']}")
        result[entry["path"]] = entry
    return result


def changed_paths(
    before: dict[str, Any],
    after: dict[str, Any],
) -> tuple[str, ...]:
    left = _entry_map(before)
    right = _entry_map(after)
    changed: set[str] = set()
    for path in left.keys() | right.keys():
        if left.get(path) == right.get(path):
            continue
        changed.add(path)
        for entry in (left.get(path), right.get(path)):
            if entry and isinstance(entry.get("previous_path"), str):
                changed.add(entry["previous_path"])
    return tuple(sorted(changed))


def verify(
    repo_raw: Path,
    plan_raw: Path,
    before_path: Path,
    after_path: Path,
) -> dict[str, Any]:
    repo = canonical_repo_root(repo_raw)
    plan, plan_relative = _plan_identity(repo, plan_raw)
    before = load(_lexical(before_path))
    after = load(_lexical(after_path))
    violations: list[dict[str, str]] = []
    identity_fields = ("repo_root", "plan", "attempt", "task_ids")
    for field in identity_fields:
        if before.get(field) != after.get(field):
            raise AttemptManifestError(f"attempt manifest {field} identity 不匹配")
    if before.get("repo_root") != str(repo) or before.get("plan") != plan_relative:
        raise AttemptManifestError("attempt manifest repo/plan identity 不匹配")
    current = _contract(plan, before["task_ids"])
    for payload in (before, after):
        if (
            payload.get("contract_hash") != current["contract_hash"]
            or payload.get("owner_paths") != current["owner_paths"]
        ):
            violations.append(
                {
                    "code": "CONTRACT_CHANGED",
                    "message": "attempt 执行期间任务契约发生变化",
                }
            )
            break
    if before.get("head") != after.get("head") or after.get("head") != head(repo):
        violations.append(
            {"code": "HEAD_CHANGED", "message": "attempt 执行期间 HEAD 发生变化"}
        )
    changed = changed_paths(before, after)
    for path in changed:
        if path == plan_relative:
            continue
        if not any(
            _owner_covers_path(owner, path)
            for owner in current["owner_paths"]
        ):
            violations.append(
                {
                    "code": "OUTSIDE_OWNER_UNION",
                    "path": path,
                    "message": f"路径越出 active task owner 并集：{path}",
                }
            )
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "execute-plan-attempt-verification",
        "passed": not violations,
        "plan": plan_relative,
        "attempt": before["attempt"],
        "task_ids": before["task_ids"],
        "owner_paths": current["owner_paths"],
        "changed_paths": list(changed),
        "violations": violations,
    }


def ensure_only_plan_changed(
    repo: Path,
    plan: Path,
    checkpoint_path: Path,
) -> None:
    checkpoint = load(checkpoint_path)
    current_entries = {
        entry.path: entry_to_dict(entry) for entry in capture_entries(repo)
    }
    current = {**checkpoint, "entries": list(current_entries.values())}
    changed = set(changed_paths(checkpoint, current))
    allowed = {plan.relative_to(repo).as_posix()}
    unexpected = sorted(changed - allowed)
    if unexpected:
        raise AttemptManifestError(
            "plan repair 期间组合树发生额外变化：" + ", ".join(unexpected)
        )
