#!/usr/bin/env python3
"""execute-plan 启动、active attempt 与唯一下一步计算。"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from attempt_manifest import capture, derive_before, verify  # noqa: E402
from execute_plan_common import (  # noqa: E402
    ExecutePlanRuntimeError,
    attempt_before,
    attempt_root,
    load_json,
    next_attempt,
    publish_action,
    relative,
    run,
    runtime_paths,
    save_state,
    task_state,
)
from execute_plan_contracts import task_contract_identities  # noqa: E402
from execute_plan_driver import (  # noqa: E402
    allocate_action,
    create_state,
    validate_state,
)
from execution_baseline import capture as capture_baseline  # noqa: E402
from execution_baseline import validate_owner_disjoint  # noqa: E402
from final_gate_contract import declared_final_gates  # noqa: E402
from verify_task_graph import parse_task_blocks  # noqa: E402


def validate_execution_context(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
) -> tuple[list[Any], dict[int, str]]:
    """每个 transition 前复核 immutable baseline 与 owner 隔离。"""

    baseline = capture_baseline(repo, plan, paths["baseline"])
    task_graph, contract_hashes = task_contract_identities(plan.read_text())
    validate_owner_disjoint(baseline, task_graph)
    if paths["state"].exists():
        state = load_json(paths["state"])
        validate_state(state)
        if state.get("phase") != "PLAN_REPAIR_REQUIRED":
            current_task_ids = {task.id for task in task_graph}
            persisted_task_ids = {
                int(task_id) for task_id in state["tasks"]
            }
            if current_task_ids != persisted_task_ids:
                raise ExecutePlanRuntimeError(
                    "task identity 在 repair 外发生变化："
                    f"plan-only={sorted(current_task_ids - persisted_task_ids)}, "
                    f"state-only={sorted(persisted_task_ids - current_task_ids)}"
                )
            changed = [
                task.id
                for task in task_graph
                if task_state(state, task.id).get("contract_hash")
                != contract_hashes[task.id]
            ]
            if changed:
                raise ExecutePlanRuntimeError(
                    "task contract 在 repair 外发生变化："
                    + ", ".join(f"任务{task_id}" for task_id in changed)
                )
    return task_graph, contract_hashes


def _dependencies_ready(state: dict[str, Any], task: dict[str, Any]) -> bool:
    return all(
        task_state(state, int(dependency))["status"] == "READY"
        for dependency in task["blocked_by"]
    )


def _validate_accepted_artifacts(repo: Path, state: dict[str, Any]) -> None:
    for task in state["tasks"].values():
        artifact_value = task.get("accepted_artifact")
        expected_hash = task.get("accepted_artifact_hash")
        if artifact_value is None and expected_hash is None:
            continue
        if not isinstance(artifact_value, str) or not isinstance(expected_hash, str):
            raise ExecutePlanRuntimeError("accepted artifact ref/hash 不完整")
        artifact = repo / artifact_value
        if (
            not artifact.is_file()
            or hashlib.sha256(artifact.read_bytes()).hexdigest() != expected_hash
        ):
            raise ExecutePlanRuntimeError("accepted artifact 缺失或 hash 已漂移")


def desired_attempt_task_ids(state: dict[str, Any]) -> list[int]:
    """当前可运行或仍有 active action 的 task owner 并集。"""

    selected: list[int] = []
    for raw_task_id, task in sorted(
        state["tasks"].items(),
        key=lambda item: int(item[0]),
    ):
        if task["status"] == "READY":
            continue
        if (
            task.get("active_action") is not None
            or _dependencies_ready(state, task)
        ):
            selected.append(int(raw_task_id))
    return selected


def start_attempt(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
    state: dict[str, Any],
    *,
    base: Path | None = None,
    accept_owner_paths: tuple[str, ...] = (),
) -> None:
    task_ids = desired_attempt_task_ids(state)
    state["attempt_task_ids"] = task_ids
    if not task_ids:
        return
    attempt = next_attempt(state)
    output = attempt_before(paths["root"], attempt)
    if base is None:
        capture(repo, plan, attempt, task_ids, output)
        state.setdefault("execution_base", relative(repo, output))
        return
    blocks = {
        block.task.id: block.task for block in parse_task_blocks(plan.read_text())
    }
    automatic_reset_paths = tuple(
        owner
        for task_id in task_ids
        if task_state(state, task_id).get("reset_to_execution_base") is True
        for owner in blocks[task_id].owned_files
    )
    derive_before(
        repo,
        plan,
        attempt,
        task_ids,
        output,
        base_path=base,
        accept_owner_paths=accept_owner_paths,
        reset_path=repo / str(state["execution_base"]),
        reset_owner_paths=automatic_reset_paths,
    )


def checkpoint_attempt(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
    state: dict[str, Any],
    *,
    label: str,
) -> Path:
    attempt = int(state["attempt"])
    task_ids = [int(task_id) for task_id in state["attempt_task_ids"]]
    checkpoint_root = attempt_root(paths["root"], attempt) / "checkpoints"
    index = len(list(checkpoint_root.glob(f"{label}-*.json"))) + 1
    checkpoint = checkpoint_root / f"{label}-{index:02d}.json"
    capture(repo, plan, attempt, task_ids, checkpoint)
    verification = verify(
        repo,
        plan,
        attempt_before(paths["root"], attempt),
        checkpoint,
    )
    if not verification["passed"]:
        details = "; ".join(
            violation["message"] for violation in verification["violations"]
        )
        raise ExecutePlanRuntimeError(f"active attempt owner 隔离失败：{details}")
    return checkpoint


def prepare_task_action(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
    state: dict[str, Any],
    task_id: int,
) -> dict[str, str]:
    if task_id not in state["attempt_task_ids"]:
        raise ExecutePlanRuntimeError("task 不属于当前 active attempt")
    task = task_state(state, task_id)
    generation = int(task["generation"]) + 1
    report_dir = (
        attempt_root(paths["root"], int(state["attempt"]))
        / f"task-{task_id}"
        / f"generation-{generation:02d}"
    )
    capture(
        repo,
        plan,
        int(state["attempt"]),
        (task_id,),
        report_dir / "generation-before.json",
    )
    snapshot = report_dir / "task-snapshot.md"
    run(
        repo,
        report_dir / "strip-plan.log",
        sys.executable,
        str(SCRIPT_DIR / "strip_plan.py"),
        str(plan),
        str(snapshot),
        "--task-id",
        str(task_id),
    )
    action_name = (
        "REMEDIATE_TASK"
        if task["status"] == "NEEDS_REFINEMENT" and task.get("latest_output")
        else "DISPATCH_TASK"
    )
    inputs = [
        relative(repo, snapshot),
        "docs/guides/plan-execution-guide.md",
        "docs/guides/task-agent-report-format.md",
    ]
    if action_name == "REMEDIATE_TASK":
        inputs.append(str(task["latest_output"]))
    if task.get("resume_context"):
        inputs.append(str(task["resume_context"]))
    completion = report_dir / "completion.txt"
    command = (
        f"retrigger original task agent for task {task_id}"
        if action_name == "REMEDIATE_TASK"
        else f"dispatch task agent for task {task_id}"
    )
    return allocate_action(
        state,
        task_id=task_id,
        action=action_name,
        generation=generation,
        input_artifacts=tuple(inputs),
        expected_output=relative(repo, completion),
        command=command,
    )


def next_action(
    repo: Path,
    plan: Path,
    plan_relative: str,
    paths: dict[str, Path],
    state: dict[str, Any],
) -> str:
    validate_state(state)
    _validate_accepted_artifacts(repo, state)
    phase = state["phase"]
    if phase == "FINALIZE":
        declarations = declared_final_gates(plan.read_text())
        gate_sequence = (
            "/".join(gate.kind for gate in declarations)
            if declarations
            else "lint/unit/build"
        )
        return (
            "EXECUTE_PLAN_STATUS\nSTATE=FINALIZE\n"
            f"NEXT=run-final {gate_sequence} -> finalize -> "
            "commit-and-push -> verify-landing -> STATE=COMPLETE\n"
        )
    if phase == "READY_TO_COMMIT":
        return (
            "EXECUTE_PLAN_STATUS\nSTATE=READY_TO_COMMIT\n"
            "NEXT=invoke commit-and-push, then verify-landing\n"
        )
    if phase == "PLAN_REPAIR_REQUIRED":
        repair = state.get("repair")
        if not isinstance(repair, dict):
            raise ExecutePlanRuntimeError("repair identity 缺失")
        producer = (
            f"\nPRODUCER_TASK_ID={repair['producer_task_id']}"
            if repair.get("producer_task_id") is not None
            else ""
        )
        return (
            "EXECUTE_PLAN_STATUS\nSTATE=PLAN_REPAIR_REQUIRED\n"
            f"TASK_ID={repair['task_id']}\n"
            f"REASON={repair['reason']}{producer}\n"
            "NEXT=STOP; resolve the structured repair, then resume-repair\n"
        )
    if phase == "COMPLETE":
        return "EXECUTE_PLAN_STATUS\nSTATE=COMPLETE\nNEXT=STOP\n"
    if phase != "TASKS":
        raise ExecutePlanRuntimeError(f"未知 execute-plan phase：{phase}")
    if all(task["status"] == "READY" for task in state["tasks"].values()):
        state["phase"] = "FINALIZE"
        save_state(paths["state"], state)
        return next_action(repo, plan, plan_relative, paths, state)
    for raw_task_id, task in sorted(
        state["tasks"].items(),
        key=lambda item: int(item[0]),
    ):
        task_id = int(raw_task_id)
        if (
            task["status"] in {"PENDING", "NEEDS_REFINEMENT"}
            and task["active_action"] is None
            and _dependencies_ready(state, task)
        ):
            action = prepare_task_action(repo, plan, paths, state, task_id)
            return publish_action(paths["root"], paths["state"], state, action)
    for task in state["tasks"].values():
        if isinstance(task.get("active_action"), dict):
            from execute_plan_driver import render_action

            return render_action(task["active_action"])
    return "EXECUTE_PLAN_STATUS\nSTATE=WAITING\nNEXT=ingest current artifact\n"


def start_execution(
    repo: Path,
    plan: Path,
    plan_relative: str,
) -> str:
    paths = runtime_paths(repo, plan)
    if paths["state"].exists():
        validate_execution_context(repo, plan, paths)
        state = load_json(paths["state"])
        return next_action(repo, plan, plan_relative, paths, state)
    start_log = paths["root"] / "main" / "start"
    run(
        repo,
        start_log / "capture.log",
        sys.executable,
        str(SCRIPT_DIR / "execution_baseline.py"),
        "--repo",
        str(repo),
        "--plan",
        str(plan),
        "--output",
        str(paths["baseline"]),
    )
    for name, script in (
        ("plan-gate.log", "plan_execution_gate.py"),
        ("graph-gate.log", "verify_task_graph.py"),
    ):
        run(
            repo,
            start_log / name,
            sys.executable,
            str(SCRIPT_DIR / script),
            str(plan),
        )
    task_graph, contract_hashes = validate_execution_context(
        repo,
        plan,
        paths,
    )
    state = create_state(
        plan=plan_relative,
        tasks=tuple(
            {
                "id": task.id,
                "name": task.name,
                "blocked_by": task.blocked_by,
                "consumes": task.consumes,
                "done": task.done,
                "contract_hash": contract_hashes[task.id],
            }
            for task in task_graph
        ),
    )
    state["attempt"] = 0
    state["attempt_task_ids"] = []
    start_attempt(repo, plan, paths, state)
    save_state(paths["state"], state)
    return next_action(repo, plan, plan_relative, paths, state)


def status(repo: Path, plan: Path) -> str:
    paths = runtime_paths(repo, plan)
    state = load_json(paths["state"])
    validate_state(state)
    counts: dict[str, int] = {}
    for task in state["tasks"].values():
        counts[task["status"]] = counts.get(task["status"], 0) + 1
    return (
        "EXECUTE_PLAN_STATUS\n"
        f"PHASE={state['phase']}\n"
        f"ATTEMPT={state.get('attempt', 0)}\n"
        f"TASKS={json.dumps(counts, sort_keys=True, separators=(',', ':'))}\n"
        f"STATE={relative(repo, paths['state'])}\n"
    )
