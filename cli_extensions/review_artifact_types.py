#!/usr/bin/env python3
"""校验 review.md，并确定性渲染开发者可读的聊天回复。"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping


ReviewMode = Literal["plan", "pr", "task"]

SUBSTANTIVE_HEADING = "## 新问题与建议"
SYNC_HEADING = "## 同步清单（CONSISTENCY drift，非阻塞）"
PREVIOUS_HEADING_TITLE = "前轮问题核销"
EXPLORATION_HEADING_TITLE = "本轮探索区域"
TITLE_RE = re.compile(
    r"^# Review Round (?P<round>\d+) — "
    r"(?P<label>计划审查|代码审查|任务验收)\s*$"
)
FIELD_RE = re.compile(r"^\*\*(?P<key>[^*]+)\*\*：(?P<value>.*)$")
H2_RE = re.compile(r"^## (?!#)(?P<title>.+?)\s*$")
FENCE_RE = re.compile(r"^\s*(?P<fence>`{3,}|~{3,})")
FINDING_ID_RE = re.compile(r"^###\s+(?P<id>[^\s.：]+)(?:[.：]|\s)")
ROUND_PATH_RE = re.compile(r"(?:^|/)round-(?P<round>\d+)(?:/review\.md)?$")
VERDICT_RE = re.compile(r"^-\s+\*\*判定\*\*：\s*(?P<verdict>.+?)\s*$")
RECOVERY_REASON_RE = re.compile(
    r"^-\s+\*\*修复原因\*\*：\s*(?P<reason>.+?)\s*$"
)
PRODUCER_TASK_RE = re.compile(
    r"^-\s+\*\*上游任务\*\*：\s*(?P<producer>.+?)\s*$"
)
SUMMARY_RE = re.compile(r"^>\s*总结：\S.*$")
EMPTY_PREVIOUS_SUMMARY = "> 总结：前轮无待核销 finding。"
PREVIOUS_TABLE_HEADER = ("#", "来源", "问题", "核销状态", "证据 / 去向")
PREVIOUS_TABLE_SEPARATOR = ("---", "------", "------", "----------", "-------------")
PREVIOUS_STATUSES = frozenset(
    {
        "satisfactory",
        "rejected",
        "withdrawn",
        "partially-addressed",
        "not-addressed",
        "disputed",
    }
)
EXPECTED_H2_TITLES: Mapping[ReviewMode, tuple[str, ...]] = {
    "plan": (
        PREVIOUS_HEADING_TITLE,
        "新问题与建议",
        "同步清单（CONSISTENCY drift，非阻塞）",
        EXPLORATION_HEADING_TITLE,
        "计划就绪状态",
    ),
    "pr": (
        PREVIOUS_HEADING_TITLE,
        "新问题与建议",
        "同步清单（CONSISTENCY drift，非阻塞）",
        EXPLORATION_HEADING_TITLE,
        "代码就绪状态",
    ),
    "task": (
        PREVIOUS_HEADING_TITLE,
        "任务契约对照",
        "新问题与建议",
        "同步清单（CONSISTENCY drift，非阻塞）",
        "验收证据",
        "任务就绪状态",
    ),
}
LEGAL_VERDICTS: Mapping[ReviewMode, set[str]] = {
    "plan": {"可执行（Ready）", "需要完善（Needs Refinement）", "废弃（Abandon）"},
    "pr": {"Ready", "Needs Refinement", "Abandon"},
    "task": {"Ready", "Needs Refinement", "Plan Repair Required"},
}
TASK_RECOVERY_REASONS = frozenset(
    {"upstream-contract", "owner-graph-contract", "developer-decision"}
)


class ReviewArtifactError(ValueError):
    """review artifact 不完整、格式非法或彼此不兼容。"""


@dataclass(frozen=True)
class Heading:
    title: str
    start: int
    body_start: int
    end: int


@dataclass(frozen=True)
class ReviewArtifact:
    path: Path
    mode: ReviewMode
    round_number: int
    lane: str
    target: Mapping[str, str]
    substantive: str
    sync: str
    finding_ids: tuple[str, ...]
    previous_summary: str | None
    verdict: str
    recovery_reason: str | None
    producer_task_id: int | None
    text: str
    headings: tuple[Heading, ...]
