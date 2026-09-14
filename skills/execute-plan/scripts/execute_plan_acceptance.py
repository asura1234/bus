#!/usr/bin/env python3
"""execute-plan task completion、review 三态与即时 downstream 解锁。"""

from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
CLI_EXTENSIONS = SCRIPT_DIR.parents[2] / "cli_extensions"
for path in (SCRIPT_DIR, CLI_EXTENSIONS):
    if str(path) not in sys.path:
        sys.path.insert(0, str(path))

from attempt_manifest import ensure_only_plan_changed  # noqa: E402
from execute_plan_common import (  # noqa: E402
    ExecutePlanRuntimeError,
    action_path,
    attempt_before,
    load_json,
    publish_action,
    relative,
    review_output,
    run,
    runtime_paths,
    save_state,
    task_state,
)
from execute_plan_driver import allocate_action, ingest_action  # noqa: E402
from execute_plan_lifecycle import (  # noqa: E402
    checkpoint_attempt,
    next_action,
    prepare_task_action,
    start_attempt,
)
from execute_plan_review_scope import (  # noqa: E402
    validate_review_scope_inputs,
    validate_task_target,
)
from review_artifact_parser import parse_review_artifact  # noqa: E402
from task_agent_report import parse_report, render_completion  # noqa: E402
from task_file_scope import (  # noqa: E402
    cumulative_task_files,
    generation_delta_files,
    publish_task_file_scope,
)
from task_graph_selection import select_task_block  # noqa: E402
from verify_task_graph import parse_task_blocks  # noqa: E402


COMPLETION_REPAIR_REASONS = {
    "OWNER_GAP": "owner-graph-contract",
    "NEEDS_CONTEXT": "needs-context",
    "BLOCKED": "developer-decision",
}


def _repair_status(
    repo: Path,
    paths: dict[str, Path],
    state: dict[str, Any],
    *,
    task_id: int,
    reason: str,
    checkpoint: Path,
    producer_task_id: int | None = None,
) -> str:
    state["phase"] = "PLAN_REPAIR_REQUIRED"
    state["repair"] = {
        "task_id": task_id,
        "reason": reason,
        "producer_task_id": producer_task_id,
        "checkpoint": relative(repo, checkpoint),
    }
    save_state(paths["state"], state)
    producer = (
        f"\nPRODUCER_TASK_ID={producer_task_id}"
        if producer_task_id is not None
        else ""
    )
    return (
        "EXECUTE_PLAN_STATUS\n"
        "STATE=PLAN_REPAIR_REQUIRED\n"
        f"TASK_ID={task_id}\n"
        f"REASON={reason}{producer}\n"
        "NEXT=STOP; resolve the structured repair, then resume-repair\n"
    )


def ingest_completion(
    repo: Path,
    plan: Path,
    plan_relative: str,
    *,
    action_id: str,
    expected_state_hash: str,
) -> str:
    paths = runtime_paths(repo, plan)
    state = load_json(paths["state"])
    action = load_json(action_path(paths["root"], action_id))
    if action.get("ACTION") not in {"DISPATCH_TASK", "REMEDIATE_TASK"}:
        raise ExecutePlanRuntimeError("当前 action 不是 task completion action")
    task_id = int(action["TASK_ID"])
    generation = int(action["GENERATION"])
    completion = repo / action["EXPECTED_OUTPUT"]
    report_path = completion.with_name("report.md")
    if not completion.is_file() or not report_path.is_file():
        raise ExecutePlanRuntimeError("当前 generation completion/report 尚未发布")
    expected = render_completion(
        parse_report(report_path.read_text()),
        relative(repo, report_path),
    )
    if completion.read_text() != expected:
        raise ExecutePlanRuntimeError("completion 内容与当前 report 不一致")
    snapshot = action["INPUT_ARTIFACTS"].split(",")[0]
    run(
        repo,
        report_path.parent / "main-validate.log",
        sys.executable,
        str(SCRIPT_DIR / "task_agent_report.py"),
        "validate",
        "--repo",
        str(repo),
        "--report",
        str(report_path),
        "--task-id",
        str(task_id),
        "--task-name",
        task_state(state, task_id)["name"],
        "--snapshot",
        snapshot,
    )
    checkpoint = checkpoint_attempt(
        repo,
        plan,
        paths,
        state,
        label=f"completion-{task_id}-{generation}",
    )
    cumulative = cumulative_task_files(
        plan,
        paths["root"],
        state,
        task_id,
        checkpoint,
    )
    delta = generation_delta_files(
        repo,
        plan,
        state,
        task_id,
        report_path.parent,
    )
    report = parse_report(report_path.read_text())
    if tuple(sorted(report.touched_files)) != tuple(sorted(delta)):
        raise ExecutePlanRuntimeError(
            "report touched files 与 driver 机械 generation delta 不一致"
        )
    file_scope_path = report_path.parent / "file-scope.json"
    publish_task_file_scope(
        repo,
        file_scope_path,
        task_id=task_id,
        generation=generation,
        report=report_path,
        generation_delta_files=delta,
        cumulative_task_files=cumulative,
    )
    if report.status not in {"DONE", "DONE_WITH_CONCERNS"}:
        ingest_action(
            state,
            action_id=action_id,
            expected_state_hash=expected_state_hash,
            next_status="PLAN_REPAIR_REQUIRED",
            output_artifact=relative(repo, report_path),
        )
        return _repair_status(
            repo,
            paths,
            state,
            task_id=task_id,
            reason=COMPLETION_REPAIR_REASONS[report.status],
            checkpoint=checkpoint,
        )
    ingest_action(
        state,
        action_id=action_id,
        expected_state_hash=expected_state_hash,
        next_status="EVIDENCE_READY",
        output_artifact=relative(repo, report_path),
    )
    output = review_output(repo, plan_relative, task_id, report.task_name)
    command = (
        "python3 skills/execute-plan/scripts/execute_plan_task_review.py "
        f"--plan {plan_relative} --task-id {task_id} "
        f"--files {' '.join(cumulative)} "
        f"--file-scope {relative(repo, file_scope_path)} "
        f"--report {relative(repo, report_path)} "
        f"--output {relative(repo, output)}"
    )
    review_action = allocate_action(
        state,
        task_id=task_id,
        action="REVIEW_TASK",
        generation=generation,
        input_artifacts=(
            plan_relative,
            relative(repo, report_path),
            relative(repo, file_scope_path),
            *cumulative,
        ),
        expected_output=relative(repo, output),
        command=command,
    )
    return publish_action(paths["root"], paths["state"], state, review_action)


def _mark_done(plan: Path, task_id: int) -> None:
    text = plan.read_text()
    block = select_task_block(parse_task_blocks(text), task_id=task_id)
    if re.search(r"(?m)^\s*-\s*\[[xX]\]\s*\*\*完成\*\*\s*$", block.text):
        return
    changed, count = re.subn(
        r"(?m)^(\s*-\s*)\[ \](\s*\*\*完成\*\*\s*)$",
        r"\1[x]\2",
        block.text,
        count=1,
    )
    if count != 1:
        raise ExecutePlanRuntimeError(f"任务{task_id} 完成 checkbox 不可唯一写回")
    plan.write_text(text.replace(block.text, changed, 1))


def ingest_review(
    repo: Path,
    plan: Path,
    plan_relative: str,
    *,
    action_id: str,
    expected_state_hash: str,
) -> str:
    paths = runtime_paths(repo, plan)
    state = load_json(paths["state"])
    action = load_json(action_path(paths["root"], action_id))
    if action.get("ACTION") != "REVIEW_TASK":
        raise ExecutePlanRuntimeError("当前 action 不是 review action")
    review = Path(action["EXPECTED_OUTPUT"])
    review = review if review.is_absolute() else repo / review
    artifact = parse_review_artifact(review)
    task_id = int(action["TASK_ID"])
    task = task_state(state, task_id)
    if artifact.mode != "task":
        raise ExecutePlanRuntimeError("review artifact 不是 task mode")
    validate_task_target(
        artifact.target,
        plan_relative=plan_relative,
        task_id=task_id,
        task_name=task["name"],
    )
    validate_review_scope_inputs(
        repo,
        review,
        action,
        artifact.target["scope_hash"],
        task_id=task_id,
        task_name=task["name"],
        plan_relative=plan_relative,
    )
    next_review_round = int(task["review_round"]) + 1
    if next_review_round > 3:
        raise ExecutePlanRuntimeError("同一 task attempt 超过三轮 review，必须停止诊断")
    if artifact.verdict == "Needs Refinement":
        ingest_action(
            state,
            action_id=action_id,
            expected_state_hash=expected_state_hash,
            next_status="NEEDS_REFINEMENT",
            output_artifact=relative(repo, review),
        )
        task["review_round"] = next_review_round
        task["accepted_artifact"] = None
        task["accepted_artifact_hash"] = None
        return publish_action(
            paths["root"],
            paths["state"],
            state,
            prepare_task_action(repo, plan, paths, state, task_id),
        )
    checkpoint = checkpoint_attempt(
        repo,
        plan,
        paths,
        state,
        label=f"review-{task_id}-{next_review_round}",
    )
    if artifact.verdict == "Plan Repair Required":
        ingest_action(
            state,
            action_id=action_id,
            expected_state_hash=expected_state_hash,
            next_status="PLAN_REPAIR_REQUIRED",
            output_artifact=relative(repo, review),
        )
        task["review_round"] = next_review_round
        return _repair_status(
            repo,
            paths,
            state,
            task_id=task_id,
            reason=artifact.recovery_reason or "owner-graph-contract",
            producer_task_id=artifact.producer_task_id,
            checkpoint=checkpoint,
        )
    ingest_action(
        state,
        action_id=action_id,
        expected_state_hash=expected_state_hash,
        next_status="READY",
        output_artifact=relative(repo, review),
    )
    task["review_round"] = next_review_round
    task["accepted_artifact"] = relative(repo, review)
    task["accepted_artifact_hash"] = hashlib.sha256(review.read_bytes()).hexdigest()
    task["retained_delta"] = False
    task["reset_to_execution_base"] = False
    current_attempt_before = attempt_before(paths["root"], int(state["attempt"]))
    owners = select_task_block(
        parse_task_blocks(plan.read_text()),
        task_id=task_id,
    ).task.owned_files
    _mark_done(plan, task_id)
    ensure_only_plan_changed(repo, plan, checkpoint)
    start_attempt(
        repo,
        plan,
        paths,
        state,
        base=current_attempt_before,
        accept_owner_paths=owners,
    )
    save_state(paths["state"], state)
    return next_action(repo, plan, plan_relative, paths, state)
