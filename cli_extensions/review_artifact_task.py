#!/usr/bin/env python3
"""Task review 结构化 repair 字段解析。"""

from __future__ import annotations

import re
from pathlib import Path

from review_artifact_types import (
    PRODUCER_TASK_RE,
    RECOVERY_REASON_RE,
    TASK_RECOVERY_REASONS,
    ReviewArtifactError,
    ReviewMode,
)


def parse_task_recovery(
    mode: ReviewMode,
    verdict: str,
    verdict_lines: list[tuple[int, str]],
    path: Path,
) -> tuple[str | None, int | None]:
    if mode != "task":
        return None, None
    reasons = [
        match.group("reason")
        for _, line in verdict_lines
        if (match := RECOVERY_REASON_RE.fullmatch(line)) is not None
    ]
    producers = [
        match.group("producer")
        for _, line in verdict_lines
        if (match := PRODUCER_TASK_RE.fullmatch(line)) is not None
    ]
    if len(reasons) != 1 or len(producers) != 1:
        raise ReviewArtifactError(
            f"task review 必须有唯一修复原因与上游任务字段: {path}"
        )
    reason = reasons[0]
    producer = producers[0]
    if verdict != "Plan Repair Required":
        if reason != "无" or producer != "无":
            raise ReviewArtifactError(
                f"非 repair task verdict 必须使用空恢复字段: {path}"
            )
        return None, None
    if reason not in TASK_RECOVERY_REASONS:
        raise ReviewArtifactError(f"task repair reason 不合法: {path}")
    if reason == "upstream-contract":
        match = re.fullmatch(r"任务\s*(\d+)", producer)
        if match is None or int(match.group(1)) <= 0:
            raise ReviewArtifactError(
                f"upstream-contract 必须声明正整数上游任务: {path}"
            )
        return reason, int(match.group(1))
    if producer != "无":
        raise ReviewArtifactError(f"非 upstream repair 不得声明上游任务: {path}")
    return reason, None
