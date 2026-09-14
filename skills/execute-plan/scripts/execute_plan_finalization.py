#!/usr/bin/env python3
"""执行并绑定 FINAL_TREE gate，随后验证 commit/push 落盘树。"""

from __future__ import annotations

import hashlib
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from execute_plan_common import (  # noqa: E402
    ExecutePlanRuntimeError,
    atomic_json,
    load_json,
    relative,
    save_state,
    set_plan_execution_status,
)
from execute_plan_lifecycle import validate_execution_context  # noqa: E402
from final_gate_contract import (  # noqa: E402
    CHANGED_LINT_PREFIX,
    GATE_KINDS,
    DeclaredFinalGate,
    declared_final_gates,
    validate_command_shape,
    validate_declared_execution,
)
from workspace_manifest import (  # noqa: E402
    atomic_create_json,
    capture_entries,
    git,
    worktree_tree_excluding,
)


PLAN_COMPLETE_RE = re.compile(
    r"(?m)^\*\*状态\*\*\s*[:：]\s*plan-execution-complete\s*$"
)
def _baseline_paths(baseline: dict[str, object]) -> tuple[str, ...]:
    result: set[str] = set()
    for entry in baseline.get("entries", []):
        if not isinstance(entry, dict):
            raise ExecutePlanRuntimeError("baseline entry schema 不合法")
        for field in ("path", "previous_path"):
            value = entry.get(field)
            if isinstance(value, str):
                result.add(value)
    return tuple(sorted(result))


def verified_worktree_tree(repo: Path, baseline: dict[str, object]) -> str:
    """计划树排除已固定且保持不变的开发者 baseline。"""

    return worktree_tree_excluding(repo, _baseline_paths(baseline))


def _validate_gate_command(
    repo: Path,
    baseline: dict[str, object],
    gate_kind: str,
    command: tuple[str, ...],
) -> str | None:
    base = validate_command_shape(
        gate_kind,
        command,
        declaration=False,
    )
    if gate_kind == "lint" and base is not None:
        try:
            resolved_base = git(repo, "rev-parse", f"{base}^{{commit}}")
        except ValueError as error:
            raise ExecutePlanRuntimeError("final lint base ref 必须可解析") from error
        if resolved_base != baseline.get("execution_start_head"):
            raise ExecutePlanRuntimeError(
                "final lint changed base 必须解析为 execution start HEAD"
            )
        return resolved_base
    return None


def _execution_gate_command(
    repo: Path,
    baseline: dict[str, object],
    gate_kind: str,
    command: tuple[str, ...],
) -> tuple[str | None, tuple[str, ...], tuple[str, ...]]:
    resolved_base = _validate_gate_command(repo, baseline, gate_kind, command)
    canonical = (
        (*CHANGED_LINT_PREFIX, resolved_base)
        if resolved_base is not None
        else command
    )
    if resolved_base is None:
        return resolved_base, canonical, canonical
    return resolved_base, canonical, canonical


def _declared_gate_map(plan: Path) -> dict[str, DeclaredFinalGate]:
    return {
        gate.kind: gate for gate in declared_final_gates(plan.read_text())
    }


def _validate_declared_gate(
    declarations: dict[str, DeclaredFinalGate],
    gate_kind: str,
    command: tuple[str, ...],
) -> None:
    if not declarations:
        return
    declared = declarations.get(gate_kind)
    if declared is None:
        raise ExecutePlanRuntimeError(
            f"计划未声明 final {gate_kind} gate"
        )
    validate_declared_execution(declared, command)


def run_final_gate(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
    *,
    gate_kind: str,
    command: tuple[str, ...],
) -> str:
    if gate_kind not in GATE_KINDS or not command:
        raise ExecutePlanRuntimeError("final gate kind/command 不合法")
    state = load_json(paths["state"])
    if state.get("phase") != "FINALIZE":
        raise ExecutePlanRuntimeError("只有 FINALIZE phase 可运行 final gate")
    validate_execution_context(repo, plan, paths)
    baseline_payload = load_json(paths["baseline"])
    declarations = _declared_gate_map(plan)
    _validate_declared_gate(declarations, gate_kind, command)
    resolved_base, command, executed_command = _execution_gate_command(
        repo,
        baseline_payload,
        gate_kind,
        command,
    )
    set_plan_execution_status(plan, "plan-execution-complete")
    tree = verified_worktree_tree(repo, baseline_payload)
    gate_root = paths["final_root"] / tree
    log = gate_root / f"{gate_kind}.log"
    evidence_path = gate_root / "gates" / f"{gate_kind}.json"
    command_text = shlex.join(command)
    executed_command_text = shlex.join(executed_command)
    if evidence_path.exists():
        existing = _successful_gate_events(
            repo,
            paths,
            tree,
            declarations,
        )
        event = existing.get(gate_kind)
        if (
            event is None
            or event.get("command") != command_text
            or event.get("executed_command") != executed_command_text
        ):
            raise ExecutePlanRuntimeError(
                f"当前 tree 的 final {gate_kind} evidence 与本次命令不一致"
            )
        return (
            "EXECUTE_PLAN_STATUS\n"
            "STATE=FINALIZE\n"
            f"TREE={tree}\n"
            f"GATE={gate_kind}\n"
            f"LOG={event['artifact']}\n"
            "NEXT=run remaining final gate or finalize\n"
        )
    started = time.monotonic()
    result = subprocess.run(
        executed_command,
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
    )
    duration = time.monotonic() - started
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text(f"$ {executed_command_text}\n" + result.stdout + result.stderr)
    if result.returncode != 0:
        set_plan_execution_status(plan, "plan-execution-in-progress")
        raise ExecutePlanRuntimeError(
            f"final {gate_kind} 失败（exit={result.returncode}）；见 {log}"
        )
    evidence = {
        "schema_version": 1,
        "kind": "execute-plan-final-gate-evidence",
        "producer": "execute_plan_finalization.run_final_gate",
        "tree": tree,
        "gate_kind": gate_kind,
        "command": command_text,
        "executed_command": executed_command_text,
        "exit_code": result.returncode,
        "duration_seconds": duration,
        "artifact": relative(repo, log),
        "artifact_hash": hashlib.sha256(log.read_bytes()).hexdigest(),
    }
    if resolved_base is not None:
        evidence["resolved_base"] = resolved_base
    if not atomic_create_json(evidence_path, evidence):
        raise ExecutePlanRuntimeError(
            f"final {gate_kind} evidence 已被并发发布；拒绝覆盖"
        )
    return (
        "EXECUTE_PLAN_STATUS\n"
        "STATE=FINALIZE\n"
        f"TREE={tree}\n"
        f"GATE={gate_kind}\n"
        f"LOG={relative(repo, log)}\n"
        "NEXT=run remaining final gate or finalize\n"
    )


def _successful_gate_events(
    repo: Path,
    paths: dict[str, Path],
    tree: str,
    declarations: dict[str, DeclaredFinalGate],
) -> dict[str, dict[str, Any]]:
    selected: dict[str, dict[str, Any]] = {}
    baseline = load_json(paths["baseline"])
    gate_root = paths["final_root"] / tree / "gates"
    for gate_kind in sorted(GATE_KINDS):
        evidence_path = gate_root / f"{gate_kind}.json"
        if not evidence_path.is_file():
            continue
        event = load_json(evidence_path)
        expected = {
            "schema_version": 1,
            "kind": "execute-plan-final-gate-evidence",
            "producer": "execute_plan_finalization.run_final_gate",
            "tree": tree,
            "gate_kind": gate_kind,
            "exit_code": 0,
        }
        if any(event.get(key) != value for key, value in expected.items()):
            raise ExecutePlanRuntimeError(
                f"final {gate_kind} evidence identity 不合法"
            )
        command = event.get("command")
        if not isinstance(command, str):
            raise ExecutePlanRuntimeError(
                f"final {gate_kind} evidence command 不合法"
            )
        parsed_command = tuple(shlex.split(command))
        _validate_declared_gate(
            declarations,
            gate_kind,
            parsed_command,
        )
        resolved_base, canonical, executed_command = _execution_gate_command(
            repo,
            baseline,
            gate_kind,
            parsed_command,
        )
        if shlex.join(canonical) != command:
            raise ExecutePlanRuntimeError(
                f"final {gate_kind} evidence command identity 不合法"
            )
        if resolved_base != event.get("resolved_base"):
            raise ExecutePlanRuntimeError(
                f"final {gate_kind} evidence base identity 不合法"
            )
        if shlex.join(executed_command) != event.get("executed_command"):
            raise ExecutePlanRuntimeError(
                f"final {gate_kind} evidence 执行范围不合法"
            )
        artifact = Path(str(event.get("artifact", "")))
        if not artifact.is_absolute():
            artifact = repo / artifact
        expected_artifact = paths["final_root"] / tree / f"{gate_kind}.log"
        if artifact.resolve() != expected_artifact.resolve():
            raise ExecutePlanRuntimeError("final gate log 路径不 canonical")
        if not artifact.is_file():
            raise ExecutePlanRuntimeError("final gate log 不存在")
        actual_hash = hashlib.sha256(artifact.read_bytes()).hexdigest()
        if actual_hash != event.get("artifact_hash"):
            raise ExecutePlanRuntimeError("final gate log hash 已漂移")
        selected[gate_kind] = event
    return selected


def finalize(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
    *,
    require_build: bool,
    manual_e2e: bool,
) -> str:
    state = load_json(paths["state"])
    if state.get("phase") != "FINALIZE":
        raise ExecutePlanRuntimeError("只有 FINALIZE phase 可固化最终树")
    if not PLAN_COMPLETE_RE.search(plan.read_text()):
        raise ExecutePlanRuntimeError("finalize 前 plan 必须是 plan-execution-complete")
    validate_execution_context(repo, plan, paths)
    baseline = load_json(paths["baseline"])
    tree = verified_worktree_tree(repo, baseline)
    declarations = _declared_gate_map(plan)
    events = _successful_gate_events(repo, paths, tree, declarations)
    if declarations:
        required = set(declarations)
        if require_build and "build" not in required:
            raise ExecutePlanRuntimeError(
                "计划未声明 final build gate，不得用 --require-build 扩张"
            )
        if manual_e2e and "e2e" in required:
            raise ExecutePlanRuntimeError(
                "计划声明了 agent-driven e2e，不得用 --manual-e2e 替代"
            )
    else:
        required = {"lint", "unit"}
        if require_build:
            required.add("build")
    missing = sorted(required - events.keys())
    if missing:
        raise ExecutePlanRuntimeError(
            "当前 tree 缺少成功 final gate：" + ", ".join(missing)
        )
    evidence = {
        "schema_version": 1,
        "kind": "execute-plan-final-evidence",
        "plan": state["plan"],
        "execution_start_head": baseline["execution_start_head"],
        "verified_worktree_tree": tree,
        "manual_e2e": "pending" if manual_e2e else "not-required",
        "gates": {
            kind: {
                key: event.get(key)
                for key in (
                    "command",
                    "executed_command",
                    "exit_code",
                    "artifact",
                    "artifact_hash",
                    "duration_seconds",
                    "resolved_base",
                )
                if event.get(key) is not None
            }
            for kind, event in sorted(events.items())
            if kind in required
        },
    }
    evidence_path = paths["final_root"] / tree / "final-evidence.json"
    if evidence_path.exists() and load_json(evidence_path) != evidence:
        raise ExecutePlanRuntimeError("当前 tree 已有冲突 final evidence")
    atomic_json(evidence_path, evidence)
    state["phase"] = "READY_TO_COMMIT"
    state["verified_worktree_tree"] = tree
    state["final_evidence"] = relative(repo, evidence_path)
    save_state(paths["state"], state)
    return (
        "EXECUTE_PLAN_STATUS\n"
        "STATE=READY_TO_COMMIT\n"
        f"VERIFIED_WORKTREE_TREE={tree}\n"
        f"EVIDENCE={relative(repo, evidence_path)}\n"
        "NEXT=invoke commit-and-push, then verify-landing\n"
    )
