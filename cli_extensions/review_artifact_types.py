#!/usr/bin/env python3
"""Validate review.md and deterministically render the developer-readable chat response."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping


ReviewMode = Literal["plan", "pr"]

SUBSTANTIVE_HEADING = "## 新问题与建议"
SYNC_HEADING = "## 同步清单（CONSISTENCY drift，非阻塞）"
PREVIOUS_HEADING_TITLE = "前轮问题核销"
EXPLORATION_HEADING_TITLE = "本轮探索区域"
TITLE_RE = re.compile(
    r"^# Review Round (?P<round>\d+) — "
    r"(?P<label>计划审查|代码审查)\s*$"
)
FIELD_RE = re.compile(r"^\*\*(?P<key>[^*]+)\*\*：(?P<value>.*)$")
H2_RE = re.compile(r"^## (?!#)(?P<title>.+?)\s*$")
FENCE_RE = re.compile(r"^\s*(?P<fence>`{3,}|~{3,})")
FINDING_ID_RE = re.compile(r"^###\s+(?P<id>[^\s.：]+)(?:[.：]|\s)")
VERDICT_RE = re.compile(r"^-\s+\*\*判定\*\*：\s*(?P<verdict>.+?)\s*$")
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
}
# A multi-purpose PR gets its own Abandon spelling so an orchestrator can route it to
# split-pr instead of treating it like an unsalvageable Abandon.
PR_SPLIT_ABANDON = "Abandon (dimension 6: multi-purpose, split with split-pr)"
LEGAL_VERDICTS: Mapping[ReviewMode, set[str]] = {
    "plan": {"可执行（Ready）", "需要完善（Needs Refinement）", "废弃（Abandon）"},
    "pr": {"Ready", "Needs Refinement", "Abandon", PR_SPLIT_ABANDON},
}


class ReviewArtifactError(ValueError):
    """A review artifact is incomplete, malformed, or incompatible with another."""


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
    text: str
    headings: tuple[Heading, ...]
