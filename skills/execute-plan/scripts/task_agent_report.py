#!/usr/bin/env python3
"""校验 task agent report，并确定性生成短 completion envelope。"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from task_agent_evidence import (
    EvidenceError,
    GateEvidence,
    existing_touched_files,
    parse_gates,
    require_scoped_lint,
    sha256_file,
    touched_content_hash,
    validate_artifacts,
)


TITLE = "# Task Agent Report"
SECTIONS = (
    "## 实现摘要",
    "## Touched files",
    "## Gate evidence",
    "## Owner gap",
    "## Concerns",
    "## Context or blocker",
)
STATUSES = frozenset({"DONE", "DONE_WITH_CONCERNS", "OWNER_GAP", "NEEDS_CONTEXT", "BLOCKED"})
NONE = "无。"
_TOUCHED_RE = re.compile(r"^- `([^`]+)`$")
_OWNER_PATH_RE = re.compile(r"^- PATH: `([^`]+)`$")
_OWNER_REASON_RE = re.compile(r"^- REASON: (.+)$")
_HASH_RE = re.compile(r"^[0-9a-f]{64}$")
_GENERATION_DIR_RE = re.compile(r"^generation-(\d{2})$")


class ReportError(ValueError):
    """报告不符合严格格式。"""


@dataclass(frozen=True)
class TaskAgentReport:
    """已完成格式与状态一致性校验的 task agent report。"""

    task_id: int
    task_name: str
    generation: int
    status: str
    snapshot: str
    snapshot_sha256: str
    touched_content_sha256: str
    summary: str
    touched_files: tuple[str, ...]
    gates: tuple[GateEvidence, ...]
    owner_gap: tuple[str, str] | None
    concerns: str | None
    context_or_blocker: str | None


def _trim_blank(lines: list[str]) -> list[str]:
    start = 0
    end = len(lines)
    while start < end and not lines[start].strip():
        start += 1
    while end > start and not lines[end - 1].strip():
        end -= 1
    return lines[start:end]


def _content(lines: list[str], name: str) -> list[str]:
    trimmed = _trim_blank(lines)
    if not trimmed or not any(line.strip() for line in trimmed):
        raise ReportError(f"{name} 不得为空；无内容时使用「{NONE}」")
    return trimmed


def _parse_metadata(lines: list[str]) -> tuple[int, str, int, str, str, str, str]:
    fields = (
        ("- TASK_ID: ", "TASK_ID"),
        ("- TASK_NAME: ", "TASK_NAME"),
        ("- GENERATION: ", "GENERATION"),
        ("- STATUS: ", "STATUS"),
        ("- SNAPSHOT: ", "SNAPSHOT"),
        ("- SNAPSHOT_SHA256: ", "SNAPSHOT_SHA256"),
        ("- TOUCHED_CONTENT_SHA256: ", "TOUCHED_CONTENT_SHA256"),
    )
    content = _trim_blank(lines)
    if len(content) != len(fields):
        raise ReportError(
            "metadata 必须按固定顺序包含 TASK_ID/TASK_NAME/GENERATION/STATUS/"
            "SNAPSHOT/SNAPSHOT_SHA256/TOUCHED_CONTENT_SHA256"
        )
    values: list[str] = []
    for line, (prefix, name) in zip(content, fields, strict=True):
        if not line.startswith(prefix):
            raise ReportError(f"metadata 第 {len(values) + 1} 行必须是 {name}")
        value = line.removeprefix(prefix)
        if not value.strip():
            raise ReportError(f"{name} 不得为空")
        values.append(value)
    try:
        task_id = int(values[0])
    except ValueError as error:
        raise ReportError("TASK_ID 必须是正整数") from error
    if task_id <= 0:
        raise ReportError("TASK_ID 必须是正整数")
    try:
        generation = int(values[2])
    except ValueError as error:
        raise ReportError("GENERATION 必须是正整数") from error
    if generation <= 0:
        raise ReportError("GENERATION 必须是正整数")
    if values[3] not in STATUSES:
        raise ReportError(f"未知 STATUS：{values[3]}")
    for name, value in (
        ("SNAPSHOT_SHA256", values[5]),
        ("TOUCHED_CONTENT_SHA256", values[6]),
    ):
        if not _HASH_RE.fullmatch(value):
            raise ReportError(f"{name} 必须是 64 位小写十六进制 hash")
    return task_id, values[1], generation, values[3], values[4], values[5], values[6]


def _validate_repo_path(value: str, field: str) -> str:
    if "\\" in value or value.endswith("/"):
        raise ReportError(f"{field} 必须是规范仓库根相对路径：{value}")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or not path.parts
        or any(part in ("", ".", "..") for part in path.parts)
        or path.as_posix() != value
    ):
        raise ReportError(f"{field} 必须是规范仓库根相对路径：{value}")
    return value


def _parse_touched(lines: list[str]) -> tuple[str, ...]:
    content = _content(lines, "Touched files")
    if content == [NONE]:
        return ()
    if NONE in content:
        raise ReportError("Touched files 不得混用「无。」与路径")
    paths: list[str] = []
    for line in content:
        match = _TOUCHED_RE.fullmatch(line)
        if match is None:
            raise ReportError(f"Touched files 行格式错误：{line}")
        paths.append(_validate_repo_path(match.group(1), "Touched files"))
    if len(paths) != len(set(paths)):
        raise ReportError("Touched files 不得重复")
    return tuple(paths)


def _parse_owner_gap(lines: list[str]) -> tuple[str, str] | None:
    content = _content(lines, "Owner gap")
    if content == [NONE]:
        return None
    if len(content) != 2:
        raise ReportError("Owner gap 必须是「无。」或固定 PATH/REASON 两行")
    path_match = _OWNER_PATH_RE.fullmatch(content[0])
    reason_match = _OWNER_REASON_RE.fullmatch(content[1])
    if path_match is None or reason_match is None:
        raise ReportError("Owner gap 必须按固定 PATH/REASON 格式填写")
    if not reason_match.group(1).strip():
        raise ReportError("Owner gap REASON 不得为空")
    return (
        _validate_repo_path(path_match.group(1), "Owner gap PATH"),
        reason_match.group(1),
    )


def _parse_gate_evidence(lines: list[str]) -> tuple[GateEvidence, ...]:
    try:
        return parse_gates(_content(lines, "Gate evidence"))
    except EvidenceError as error:
        raise ReportError(str(error)) from error


def _parse_optional_text(lines: list[str], name: str) -> str | None:
    content = _content(lines, name)
    if content == [NONE]:
        return None
    if NONE in content:
        raise ReportError(f"{name} 不得混用「无。」与正文")
    return "\n".join(content)


def _validate_status(report: TaskAgentReport) -> None:
    completed = report.status in {"DONE", "DONE_WITH_CONCERNS"}
    if completed and (not report.gates or any(gate.exit_code != 0 for gate in report.gates)):
        raise ReportError(f"{report.status} 必须至少有一个 gate，且全部 EXIT_CODE=0")
    if report.status == "DONE" and report.concerns is not None:
        raise ReportError("DONE 的 Concerns 必须为「无。」")
    if report.status == "DONE_WITH_CONCERNS" and report.concerns is None:
        raise ReportError("DONE_WITH_CONCERNS 必须填写 Concerns")
    if report.status == "OWNER_GAP":
        if report.owner_gap is None:
            raise ReportError("OWNER_GAP 必须填写 Owner gap")
    elif report.owner_gap is not None:
        raise ReportError(f"{report.status} 的 Owner gap 必须为「无。」")
    needs_blocker = report.status in {"NEEDS_CONTEXT", "BLOCKED"}
    if needs_blocker != (report.context_or_blocker is not None):
        expected = "填写正文" if needs_blocker else "使用「无。」"
        raise ReportError(f"{report.status} 的 Context or blocker 必须{expected}")


def parse_report(text: str) -> TaskAgentReport:
    """解析并校验一个完整 report.md。"""

    lines = text.replace("\r\n", "\n").replace("\r", "\n").splitlines()
    if not lines or lines[0] != TITLE:
        raise ReportError(f"首行必须是 {TITLE}")
    for heading in SECTIONS:
        if lines.count(heading) != 1:
            raise ReportError(f"{heading} 必须且只能出现一次")
    unknown_h2 = [line for line in lines if line.startswith("## ") and line not in SECTIONS]
    if unknown_h2:
        raise ReportError(f"存在未知二级标题：{unknown_h2[0]}")
    indices = [lines.index(heading) for heading in SECTIONS]
    if indices != sorted(indices):
        raise ReportError("section 顺序不符合 task-agent-report-format.md")
    gate_start = indices[2]
    gate_end = indices[3]
    for index, line in enumerate(lines[1:], start=1):
        if line.startswith("# ") or (line.startswith("### ") and not gate_start < index < gate_end):
            raise ReportError(f"存在未授权标题：{line}")
    (
        task_id,
        task_name,
        generation,
        status,
        snapshot,
        snapshot_sha256,
        touched_content_sha256,
    ) = _parse_metadata(lines[1 : indices[0]])
    bodies: dict[str, list[str]] = {}
    for index, heading in enumerate(SECTIONS):
        end = indices[index + 1] if index + 1 < len(indices) else len(lines)
        bodies[heading] = lines[indices[index] + 1 : end]
    summary = "\n".join(_content(bodies["## 实现摘要"], "实现摘要"))
    report = TaskAgentReport(
        task_id=task_id,
        task_name=task_name,
        generation=generation,
        status=status,
        snapshot=snapshot,
        snapshot_sha256=snapshot_sha256,
        touched_content_sha256=touched_content_sha256,
        summary=summary,
        touched_files=_parse_touched(bodies["## Touched files"]),
        gates=_parse_gate_evidence(bodies["## Gate evidence"]),
        owner_gap=_parse_owner_gap(bodies["## Owner gap"]),
        concerns=_parse_optional_text(bodies["## Concerns"], "Concerns"),
        context_or_blocker=_parse_optional_text(bodies["## Context or blocker"], "Context or blocker"),
    )
    _validate_status(report)
    return report


def validate_identity(report: TaskAgentReport, task_id: int, task_name: str, snapshot: str) -> None:
    """核对主 agent 派发的 task identity。"""

    if report.task_id != task_id:
        raise ReportError(f"TASK_ID 不匹配：期望 {task_id}，实际 {report.task_id}")
    if report.task_name != task_name:
        raise ReportError(f"TASK_NAME 不匹配：期望 {task_name}，实际 {report.task_name}")
    if report.snapshot != snapshot:
        raise ReportError(f"SNAPSHOT 不匹配：期望 {snapshot}，实际 {report.snapshot}")


def render_completion(report: TaskAgentReport, report_path: str) -> str:
    """生成固定七行 completion envelope。"""

    passed = sum(gate.exit_code == 0 for gate in report.gates)
    concerns = "present" if report.concerns is not None else "none"
    return "\n".join(
        (
            "TASK_AGENT_COMPLETION",
            f"TASK_ID={report.task_id}",
            f"STATUS={report.status}",
            f"DELTA_FILES={len(report.touched_files)}",
            f"GATES={passed}/{len(report.gates)}",
            f"CONCERNS={concerns}",
            f"REPORT={report_path}",
            "",
        )
    )


def _atomic_write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as handle:
        temporary = Path(handle.name)
        handle.write(text)
        handle.flush()
        os.fsync(handle.fileno())
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _load_and_validate(args: argparse.Namespace) -> TaskAgentReport:
    report_path = Path(args.report)
    if report_path.name != "report.md":
        raise ReportError("report 文件名必须是 report.md")
    try:
        report = parse_report(report_path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ReportError(f"无法读取 report：{error}") from error
    validate_identity(report, args.task_id, args.task_name, args.snapshot)
    report_dir = report_path.resolve().parent
    generation = _GENERATION_DIR_RE.fullmatch(report_dir.name)
    if generation is None or int(generation.group(1)) != report.generation:
        raise ReportError("GENERATION 必须与 report generation-NN 目录一致")
    repo = Path(args.repo).resolve()
    snapshot = Path(report.snapshot)
    if not snapshot.is_absolute():
        snapshot = repo / snapshot
    completed = report.status in {"DONE", "DONE_WITH_CONCERNS"}
    try:
        validate_artifacts(
            repo=repo,
            report_dir=report_dir,
            snapshot=snapshot,
            snapshot_sha256=report.snapshot_sha256,
            touched_files=report.touched_files,
            touched_sha256=report.touched_content_sha256,
            gates=report.gates,
            require_contract_gates=completed,
        )
        if completed:
            require_scoped_lint(
                existing_touched_files(repo, report.touched_files),
                report.gates,
            )
    except (EvidenceError, OSError) as error:
        raise ReportError(str(error)) from error
    return report


def _add_identity_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--repo", required=True)
    parser.add_argument("--report", required=True)
    parser.add_argument("--task-id", required=True, type=int)
    parser.add_argument("--task-name", required=True)
    parser.add_argument("--snapshot", required=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate_parser = subparsers.add_parser("validate")
    _add_identity_arguments(validate_parser)
    render_parser = subparsers.add_parser("render-completion")
    _add_identity_arguments(render_parser)
    render_parser.add_argument("--output", required=True)
    fingerprint_parser = subparsers.add_parser("fingerprint")
    fingerprint_parser.add_argument("--repo", required=True)
    fingerprint_parser.add_argument("--snapshot", required=True)
    fingerprint_parser.add_argument("--file", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        if args.command == "fingerprint":
            repo = Path(args.repo).resolve()
            snapshot = Path(args.snapshot)
            if not snapshot.is_absolute():
                snapshot = repo / snapshot
            files = tuple(_validate_repo_path(value, "--file") for value in args.file)
            print(f"SNAPSHOT_SHA256={sha256_file(snapshot)}")
            print(f"TOUCHED_CONTENT_SHA256={touched_content_hash(repo, files)}")
            return 0
        report = _load_and_validate(args)
        if args.command == "validate":
            print(f"PASS TASK_ID={report.task_id} STATUS={report.status}")
            return 0
        output = Path(args.output)
        report_parent = Path(args.report).parent.resolve()
        if output.name != "completion.txt" or output.parent.resolve() != report_parent:
            raise ReportError("completion 必须是 report.md 同目录的 completion.txt")
        completion = render_completion(report, args.report)
        _atomic_write(output, completion)
        print(completion, end="")
        return 0
    except ReportError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
