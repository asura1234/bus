#!/usr/bin/env python3
"""为 execute-plan 内部 task review 校验边界并创建 round bookkeeping。"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
sys.path.insert(0, str(REPO_ROOT / "skills" / "execute-plan" / "scripts"))
from task_agent_report import parse_report  # noqa: E402
from task_file_scope import load_task_file_scope  # noqa: E402
from task_graph_selection import select_task_block  # noqa: E402
from task_review_evidence import (  # noqa: E402
    TaskEvidenceError,
    ValidatedTaskEvidence,
    validate_task_evidence,
)
from task_scope_evidence import (  # noqa: E402
    canonical_context,
    extract_section,
    extract_title,
    scope_hashes,
)
from verify_task_graph import (  # noqa: E402
    ParsedTaskBlock,
    check_plan_path_closure,
    check_task_fields,
    file_contract_for_owners,
    parse_task_blocks,
    verify,
)


_TRIAGE_MODE_RE = re.compile(
    r"(?im)^\s*(?:\*\*)?(?:模式|review type|review mode)(?:\*\*)?"
    r"\s*[:：]\s*(\S+)\s*$"
)
_TRIAGE_PLAN_RE = re.compile(r"(?im)^\s*(?:\*\*)?(?:计划|plan)(?:\*\*)?\s*[:：]\s*(\S+)\s*$")
_TRIAGE_TASK_RE = re.compile(
    r"(?im)^\s*(?:\*\*)?(?:任务|task)(?:\*\*)?\s*[:：]\s*"
    r"(?:任务\s*)?(\d+)\s*[:：]\s*(.+?)\s*$"
)


@dataclass(frozen=True)
class TaskContract:
    task_id: int
    name: str
    owned_files: tuple[str, ...]
    consumes: tuple[int, ...]
    block: str


class ValidationError(Exception):
    """调用形状合法，但任务验收机械前提不成立。"""


def _select_task(
    parsed_blocks: list[ParsedTaskBlock],
    *,
    task_id: int | None = None,
    task_name: str | None = None,
) -> TaskContract:
    try:
        selected = select_task_block(parsed_blocks, task_id=task_id, task_name=task_name)
    except ValueError as exc:
        if task_name is not None and "不存在" in str(exc):
            raise ValidationError(f"任务不存在：{task_name}") from exc
        raise ValidationError(str(exc)) from exc
    return TaskContract(
        selected.task.id,
        selected.task.name,
        selected.task.owned_files,
        selected.task.consumes,
        selected.text,
    )


def slug(value: str) -> str:
    normalized = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    return normalized or "task"


def rel(path: Path) -> str:
    try:
        return path.resolve().relative_to(REPO_ROOT.resolve()).as_posix()
    except ValueError:
        return str(path.resolve())


def _repo_path(raw: str | Path, *, require_file: bool) -> tuple[Path, str]:
    candidate = Path(raw)
    if not candidate.is_absolute():
        candidate = REPO_ROOT / candidate
    lexical_root = Path(os.path.abspath(REPO_ROOT))
    lexical = Path(os.path.abspath(candidate.expanduser()))
    try:
        relative = lexical.relative_to(lexical_root).as_posix()
    except ValueError as exc:
        raise ValidationError(f"路径越出仓库：{raw}") from exc
    try:
        lexical.parent.resolve().relative_to(REPO_ROOT.resolve())
    except ValueError as exc:
        raise ValidationError(f"路径父目录越出仓库：{raw}") from exc
    if require_file:
        resolved = lexical.resolve()
        try:
            relative = resolved.relative_to(REPO_ROOT.resolve()).as_posix()
        except ValueError as exc:
            raise ValidationError(f"路径越出仓库：{raw}") from exc
        if not resolved.is_file():
            raise ValidationError(f"文件不存在：{relative}")
        return resolved, relative
    return lexical, relative


def _is_owned(path: str, owners: tuple[str, ...]) -> bool:
    candidate = PurePosixPath(path)
    for owner in owners:
        normalized = PurePosixPath(owner.rstrip("/"))
        if candidate == normalized or normalized in candidate.parents:
            return True
    return False


def _canonical_contract(plan_text: str, task: TaskContract) -> str:
    parts = [
        extract_title(plan_text),
        extract_section(plan_text, "目标"),
        extract_section(plan_text, "非目标"),
        extract_section(plan_text, "已归档的决策"),
        extract_section(plan_text, "需要修改/添加的文件"),
        task.block,
    ]
    return "\n".join(part.rstrip() for part in parts) + "\n"


def _triage_identity(path: Path) -> tuple[str, str, int, str] | None:
    try:
        text = path.read_text()
    except OSError:
        return None
    mode = _TRIAGE_MODE_RE.search(text)
    plan = _TRIAGE_PLAN_RE.search(text)
    task = _TRIAGE_TASK_RE.search(text)
    if not (mode and plan and task):
        return None
    return mode.group(1).lower(), plan.group(1), int(task.group(1)), task.group(2).strip()


def triage_ledgers(plan_relative: str, task: TaskContract) -> list[Path]:
    root = REPO_ROOT / "temp" / "address-review-comments" / "__task__"
    if not root.is_dir():
        return []
    matches = [
        path
        for path in root.rglob("triage.md")
        if _triage_identity(path) == ("task", plan_relative, task.task_id, task.name)
    ]
    return sorted(matches, key=lambda path: (path.stat().st_mtime_ns, str(path)))


def _round_number(path: Path) -> int | None:
    match = re.fullmatch(r"round-(\d{2})", path.name)
    return int(match.group(1)) if match else None


def _plan_slug(plan_relative: str) -> str:
    plan_without_suffix = PurePosixPath(plan_relative).with_suffix("").as_posix()
    return slug(plan_without_suffix)


def _round_context(plan_relative: str, task: TaskContract, output: Path) -> tuple[int, list[Path]]:
    lane = (
        REPO_ROOT / "temp" / "review-task" / _plan_slug(plan_relative) / f"task-{task.task_id}-{slug(task.name)}"
    ).resolve()
    completed = sorted(
        path
        for path in lane.glob("round-*")
        if path.is_dir() and _round_number(path) is not None and (path / "review.md").is_file()
    )
    completed_numbers = [_round_number(path) for path in completed]
    expected_completed = list(range(1, len(completed) + 1))
    if completed_numbers != expected_completed:
        raise ValidationError("本 task lane 的已完成 round 不连续")
    round_number = len(completed) + 1
    expected = lane / f"round-{round_number:02d}" / "review.md"
    if output.resolve() != expected:
        raise ValidationError(
            f"输出路径不属于当前任务下一轮；下一轮必须是 round-{round_number:02d}：{expected.relative_to(REPO_ROOT)}"
        )
    return round_number, [path / "review.md" for path in completed]


def _write_bookkeeping(
    output: Path,
    contract: str,
    plan_relative: str,
    task: TaskContract,
    files: list[tuple[Path, str]],
    digest: str,
    context_hash: str,
    evidence: ValidatedTaskEvidence,
    file_scope: str,
) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    (output.parent / "task-contract-snapshot.md").write_text(contract)
    payload = {
        "files": [relative for _, relative in sorted(files, key=lambda item: item[1])],
        "plan": plan_relative,
        "report": rel(evidence.report_path),
        "report_hash": evidence.evidence_hash,
        "scope_hash": digest,
        "plan_context_hash": context_hash,
        "task_evidence_hash": digest,
        "task_id": task.task_id,
        "task_name": task.name,
        "file_scope": file_scope,
    }
    (output.parent / "scope-inputs.json").write_text(
        json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plan", required=True)
    selector = parser.add_mutually_exclusive_group(required=True)
    selector.add_argument("--task-id", type=int)
    selector.add_argument("--task")
    parser.add_argument("--files", nargs="*", required=True)
    parser.add_argument("--file-scope", required=True)
    parser.add_argument("--report", required=True)
    parser.add_argument("--output", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args((argv or sys.argv)[1:])
    try:
        plan, plan_relative = _repo_path(args.plan, require_file=True)
        plan_text = plan.read_text()
        task_errors = check_task_fields(plan_text)
        if task_errors:
            raise ValidationError("计划任务结构无效：" + "；".join(task_errors))
        try:
            parsed_blocks = parse_task_blocks(plan_text)
        except ValueError as exc:
            raise ValidationError(f"计划任务结构无效：{exc}") from exc
        tasks = [block.task for block in parsed_blocks]
        graph_errors = verify(tasks)
        graph_errors.extend(check_plan_path_closure(plan_text, tasks))
        if graph_errors:
            raise ValidationError("计划任务图无效：" + "；".join(graph_errors))
        task = _select_task(parsed_blocks, task_id=args.task_id, task_name=args.task)
        files = [_repo_path(raw, require_file=False) for raw in args.files]
        directories = [
            relative for path, relative in files if not path.is_symlink() and path.exists() and not path.is_file()
        ]
        if directories:
            raise ValidationError("实际触及路径必须是文件或已删除文件：" + ", ".join(directories))
        duplicate_files = len({relative for _, relative in files}) != len(files)
        if duplicate_files:
            raise ValidationError("实际触及文件列表含重复路径")
        outside = [relative for _, relative in files if not _is_owned(relative, task.owned_files)]
        if outside:
            raise ValidationError("实际触及文件越出拥有文件：" + ", ".join(sorted(outside)))
        report_path, _ = _repo_path(args.report, require_file=True)
        report = parse_report(report_path.read_text())
        file_scope_path, file_scope_relative = _repo_path(
            args.file_scope,
            require_file=True,
        )
        file_scope = load_task_file_scope(
            REPO_ROOT.resolve(),
            file_scope_path,
            report=report_path,
            task_id=task.task_id,
            generation=report.generation,
        )
        generation_files = tuple(file_scope["generation_delta_files"])
        if sorted(relative for _, relative in files) != file_scope[
            "cumulative_task_files"
        ]:
            raise ValidationError(
                "--files 与 driver file-scope cumulative files 不一致"
            )
        evidence = validate_task_evidence(
            repo=REPO_ROOT.resolve(),
            report_path=report_path,
            task_id=task.task_id,
            task_name=task.name,
            generation_files=generation_files,
        )
        output = Path(args.output)
        if not output.is_absolute():
            output = REPO_ROOT / output
        round_number, previous = _round_context(plan_relative, task, output)
        contract = _canonical_contract(plan_text, task)
        context = canonical_context(plan_text)
        producer_blocks = {block.task.id: block.text for block in parsed_blocks}
        context_hash, digest = scope_hashes(
            context=context,
            owner_file_contract=file_contract_for_owners(plan_text, task.owned_files),
            task_block=task.block,
            producer_blocks=tuple(producer_blocks[task_id] for task_id in task.consumes),
            files=files,
            evidence_hash=evidence.evidence_hash,
        )
        triage = triage_ledgers(plan_relative, task)
        _write_bookkeeping(
            output,
            contract,
            plan_relative,
            task,
            files,
            digest,
            context_hash,
            evidence,
            file_scope_relative,
        )
    except (OSError, UnicodeError, ValueError, ValidationError, TaskEvidenceError) as exc:
        print("FAIL")
        print(f"- {exc}")
        return 1

    print(f"PLAN={plan_relative}")
    print(f"TASK_ID={task.task_id}")
    print(f"TASK_NAME={task.name}")
    print("FILES=" + ",".join(relative for _, relative in sorted(files, key=lambda item: item[1])))
    print(f"FILE_COUNT={len(files)}")
    print(f"SCOPE_INPUTS={rel(output.parent / 'scope-inputs.json')}")
    print(f"REPORT={rel(evidence.report_path)}")
    print(f"GATE_EVIDENCE_HASH={evidence.evidence_hash}")
    print(f"PLAN_CONTEXT_HASH={context_hash}")
    print(f"TASK_EVIDENCE_HASH={digest}")
    print(f"SCOPE_HASH={digest}")
    print(f"ROUND={round_number}")
    print(f"OUTPUT={rel(output)}")
    print("PREV_REVIEWS=" + (",".join(rel(path) for path in previous) if previous else "none"))
    print("TRIAGE_LEDGER=" + (",".join(rel(path) for path in triage) if triage else "none"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
