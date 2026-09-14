"""把 task agent report 绑定到 execute-plan 内部 task review 输入。"""

from __future__ import annotations

import hashlib
import re
import sys
from dataclasses import dataclass
from pathlib import Path


EXECUTE_SCRIPTS = Path(__file__).resolve().parents[2] / "execute-plan" / "scripts"
if str(EXECUTE_SCRIPTS) not in sys.path:
    sys.path.insert(0, str(EXECUTE_SCRIPTS))

from task_agent_evidence import EvidenceError, validate_artifacts  # noqa: E402
from task_agent_report import ReportError, TaskAgentReport, parse_report  # noqa: E402


_GENERATION_RE = re.compile(r"^generation-(\d{2})$")


class TaskEvidenceError(ValueError):
    """Report 与当前 task review 输入不一致。"""


@dataclass(frozen=True)
class ValidatedTaskEvidence:
    report: TaskAgentReport
    report_path: Path
    evidence_hash: str


def _snapshot_path(repo: Path, value: str) -> Path:
    path = Path(value)
    return path if path.is_absolute() else repo / path


def validate_task_evidence(
    *,
    repo: Path,
    report_path: Path,
    task_id: int,
    task_name: str,
    generation_files: tuple[str, ...],
) -> ValidatedTaskEvidence:
    """校验 report identity、内容证据和 generation delta。"""

    resolved = report_path.resolve()
    if resolved.name != "report.md":
        raise TaskEvidenceError("task evidence 文件名必须是 report.md")
    generation = _GENERATION_RE.fullmatch(resolved.parent.name)
    if generation is None:
        raise TaskEvidenceError("task evidence 必须位于 generation-NN 目录")
    try:
        report = parse_report(resolved.read_text())
    except (OSError, UnicodeError, ReportError) as error:
        raise TaskEvidenceError(f"task evidence 无效：{error}") from error
    if report.task_id != task_id or report.task_name != task_name:
        raise TaskEvidenceError("task evidence identity 与选中任务不匹配")
    if report.generation != int(generation.group(1)):
        raise TaskEvidenceError("task evidence GENERATION 与目录不匹配")
    if report.status not in {"DONE", "DONE_WITH_CONCERNS"}:
        raise TaskEvidenceError(f"task evidence STATUS 不可验收：{report.status}")
    if tuple(sorted(report.touched_files)) != tuple(
        sorted(generation_files)
    ):
        raise TaskEvidenceError(
            "task evidence touched files 与机械 generation delta 不一致"
        )
    snapshot = _snapshot_path(repo, report.snapshot)
    try:
        validate_artifacts(
            repo=repo,
            report_dir=resolved.parent,
            snapshot=snapshot,
            snapshot_sha256=report.snapshot_sha256,
            touched_files=report.touched_files,
            touched_sha256=report.touched_content_sha256,
            gates=report.gates,
        )
    except (EvidenceError, OSError) as error:
        raise TaskEvidenceError(f"task evidence artifact 无效：{error}") from error
    return ValidatedTaskEvidence(
        report=report,
        report_path=resolved,
        evidence_hash=hashlib.sha256(resolved.read_bytes()).hexdigest(),
    )
