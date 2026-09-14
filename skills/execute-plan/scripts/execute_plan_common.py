#!/usr/bin/env python3
"""execute-plan runtime 共用的路径、持久化和机械命令边界。"""

from __future__ import annotations

import json
import os
import re
import subprocess
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[2]
PLAN_EXECUTION_STATUS_RE = re.compile(
    r"(?m)^\*\*状态\*\*\s*[:：]\s*"
    r"(?P<status>plan-execution-in-progress|plan-execution-complete)\s*$"
)


class ExecutePlanRuntimeError(ValueError):
    """execute-plan runtime 无法安全推进。"""


def atomic_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(payload, stream, ensure_ascii=False, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def load_json(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ExecutePlanRuntimeError(f"无法读取 JSON：{path}") from error
    if not isinstance(payload, dict):
        raise ExecutePlanRuntimeError(f"JSON 顶层必须是 object：{path}")
    return payload


def repo_plan(repo: Path, raw: str) -> tuple[Path, str]:
    candidate = Path(raw)
    plan = (candidate if candidate.is_absolute() else repo / candidate).resolve()
    try:
        relative = plan.relative_to(repo).as_posix()
    except ValueError as error:
        raise ExecutePlanRuntimeError("plan 必须位于 repo 内") from error
    if not plan.is_file():
        raise ExecutePlanRuntimeError(f"plan 不存在：{relative}")
    return plan, relative


def runtime_paths(repo: Path, plan: Path) -> dict[str, Path]:
    root = repo / "temp" / "execute-plan" / plan.stem
    return {
        "root": root,
        "state": root / "driver-state.json",
        "baseline": root / "dirty-baseline.json",
        "final_root": root / "final",
        "landing": root / "landing-proof.json",
    }


def run(repo: Path, log: Path, *command: str) -> str:
    result = subprocess.run(
        command,
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
    )
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text(result.stdout + result.stderr)
    if result.returncode != 0:
        raise ExecutePlanRuntimeError(
            f"机械命令失败（exit={result.returncode}）：{' '.join(command)}；见 {log}"
        )
    return result.stdout


def relative(repo: Path, path: Path) -> str:
    try:
        return path.resolve().relative_to(repo).as_posix()
    except ValueError as error:
        raise ExecutePlanRuntimeError(f"artifact 必须位于 repo 内：{path}") from error


def set_plan_execution_status(plan: Path, status: str) -> bool:
    """在 final gate 前后切换计划状态；返回是否发生写入。"""

    if status not in {"plan-execution-in-progress", "plan-execution-complete"}:
        raise ExecutePlanRuntimeError(f"不合法的 plan execution status：{status}")
    text = plan.read_text()
    match = PLAN_EXECUTION_STATUS_RE.search(text)
    if match is None:
        raise ExecutePlanRuntimeError(
            "计划缺少 plan-execution-in-progress/complete 状态"
        )
    if match.group("status") == status:
        return False
    changed = (
        text[: match.start("status")]
        + status
        + text[match.end("status") :]
    )
    descriptor, name = tempfile.mkstemp(
        dir=plan.parent,
        prefix=f".{plan.name}.",
    )
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(changed)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, plan.stat().st_mode)
        os.replace(temporary, plan)
    finally:
        temporary.unlink(missing_ok=True)
    return True


def attempt_root(root: Path, attempt: int) -> Path:
    if attempt <= 0:
        raise ExecutePlanRuntimeError("attempt 必须是正整数")
    return root / "attempts" / f"attempt-{attempt:02d}"


def attempt_before(root: Path, attempt: int) -> Path:
    return attempt_root(root, attempt) / "before.json"


def next_attempt(state: dict[str, Any]) -> int:
    attempt = int(state.get("attempt", 0)) + 1
    state["attempt"] = attempt
    return attempt


def task_state(state: dict[str, Any], task_id: int) -> dict[str, Any]:
    task = state["tasks"].get(str(task_id))
    if not isinstance(task, dict):
        raise ExecutePlanRuntimeError(f"task 不存在：{task_id}")
    return task


def action_path(root: Path, action_id: str) -> Path:
    return root / "actions" / f"{action_id}.json"


def save_state(state_path: Path, state: dict[str, Any]) -> None:
    from execute_plan_driver import validate_state

    validate_state(state)
    atomic_json(state_path, state)


def publish_action(
    root: Path,
    state_path: Path,
    state: dict[str, Any],
    action: dict[str, str],
) -> str:
    from execute_plan_driver import render_action

    atomic_json(action_path(root, action["ACTION_ID"]), action)
    save_state(state_path, state)
    return render_action(action)


def review_output(repo: Path, plan_relative: str, task_id: int, name: str) -> Path:
    plan_slug = re.sub(
        r"[^a-z0-9]+",
        "-",
        PurePosixPath(plan_relative).with_suffix("").as_posix().lower(),
    ).strip("-")
    name_slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-") or "task"
    lane = repo / "temp" / "review-task" / plan_slug / f"task-{task_id}-{name_slug}"
    completed = [path for path in lane.glob("round-*/review.md") if path.is_file()]
    return lane / f"round-{len(completed) + 1:02d}" / "review.md"
