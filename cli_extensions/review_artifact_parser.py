#!/usr/bin/env python3
"""Canonical parser, structural validation and deterministic renderer for review.md."""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Mapping, Sequence


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from review_artifact_text import (
    _fence_scan,
    _normalized_for_comparison,
    outside_fence_lines,
)
from review_artifact_types import (
    EMPTY_PREVIOUS_SUMMARY,
    EXPECTED_H2_TITLES,
    EXPLORATION_HEADING_TITLE,
    FIELD_RE,
    FINDING_ID_RE,
    H2_RE,
    LEGAL_VERDICTS,
    PREVIOUS_HEADING_TITLE,
    PREVIOUS_STATUSES,
    PREVIOUS_TABLE_HEADER,
    PREVIOUS_TABLE_SEPARATOR,
    SUBSTANTIVE_HEADING,
    SUMMARY_RE,
    SYNC_HEADING,
    TITLE_RE,
    VERDICT_RE,
    Heading,
    ReviewArtifact,
    ReviewArtifactError,
    ReviewMode,
)


def _parse_title(lines: list[tuple[int, str]], path: Path) -> tuple[ReviewMode, int]:
    first_nonempty = next((line for _, line in lines if line.strip()), "")
    match = TITLE_RE.fullmatch(first_nonempty)
    if match is None:
        raise ReviewArtifactError(f"无法识别 review mode/title: {path}")
    label_to_mode: Mapping[str, ReviewMode] = {
        "计划审查": "plan",
        "代码审查": "pr",
    }
    return label_to_mode[match.group("label")], int(match.group("round"))


def _parse_header_fields(text: str, lines: list[tuple[int, str]], path: Path) -> dict[str, str]:
    """
    Parse the header's `**key**：value` fields. **A value may continue on following lines**: a field
    extends until the next field line, the first H2, or the end of the header.

    Earlier this skipped every line not matching `FIELD_RE` with `continue`, so a wrapped value was
    **silently truncated to its first line** with no error. The cost in practice: two review lanes wrote
    the same "locked goal", one as a single line and the other with normal Markdown wrapping plus a bullet
    list; the latter lost 14 lines, leaving only the opening half-sentence, was then compared verbatim
    with the former, and finally stopped on a `locked goal conflict` that hid the real cause — the real
    problem being that the locked goal (the authority of the GOAL & SCOPE GATE) had already been quietly
    cut by more than half; a single-lane run hits it too, it just does not report an error.
    """
    fields: dict[str, str] = {}
    raw_lines = text.splitlines()
    key: str | None = None
    value = ""
    start = 0

    def flush(end: int) -> None:
        if key is None:
            return
        fields[key] = "\n".join([value, *raw_lines[start + 1 : end]]).strip()

    for index, line in lines:
        if H2_RE.fullmatch(line):
            flush(index)
            return fields
        match = FIELD_RE.fullmatch(line)
        if match is None:
            continue
        # Structural boundaries come only from lines outside fences; the value is cut from the raw
        # text so fenced requirements inside the goal are not lost.
        flush(index)
        key = match.group("key").strip()
        if key in fields:
            raise ReviewArtifactError(f"header field 重复: {key} ({path})")
        value = match.group("value")
        start = index
    flush(len(raw_lines))
    return fields


def _require_field(fields: Mapping[str, str], key: str, path: Path) -> str:
    value = fields.get(key)
    if not value:
        raise ReviewArtifactError(f"缺少或为空的 header field: {key} ({path})")
    return value


def _target_and_lane(
    mode: ReviewMode,
    fields: Mapping[str, str],
    title_round: int,
    path: Path,
) -> tuple[Mapping[str, str], str]:
    if mode == "plan":
        return {"plan": _require_field(fields, "计划", path)}, _require_field(fields, "审查者", path)
    if mode == "pr":
        return {
            "branch": _require_field(fields, "分支", path),
            "base": _require_field(fields, "基线", path),
            "plan": _require_field(fields, "计划（如有）", path),
            "locked_goal": _require_field(fields, "锁定目标", path),
        }, _require_field(fields, "审查者", path)
    raise ReviewArtifactError(f"不支持的 review mode: {mode} ({path})")


def _validate_path_round(path: Path, title_round: int) -> None:
    for part in path.parts:
        match = re.fullmatch(r"round-(\d+)", part)
        if match is not None and int(match.group(1)) != title_round:
            raise ReviewArtifactError(f"file path round 与 title round 不一致: {path}")


def _headings(text: str, path: Path) -> tuple[Heading, ...]:
    starts: list[tuple[str, int, int]] = []
    for _, offset, line in _fence_scan(text, path):
        heading = H2_RE.fullmatch(line.rstrip("\r\n"))
        if heading is not None:
            starts.append((heading.group("title"), offset, offset + len(line)))
    return tuple(
        Heading(
            title=title,
            start=start,
            body_start=body_start,
            end=starts[index + 1][1] if index + 1 < len(starts) else len(text),
        )
        for index, (title, start, body_start) in enumerate(starts)
    )


def _body(text: str, heading: Heading, path: Path) -> str:
    body = text[heading.body_start : heading.end].strip()
    if not body:
        raise ReviewArtifactError(f"固定 section 正文不能为空: {heading.title} ({path})")
    return body


def _validate_previous_section(
    text: str,
    heading: Heading,
    path: Path,
    round_number: int,
) -> str | None:
    body = _body(text, heading, path)
    if round_number == 1:
        if body != "无。":
            raise ReviewArtifactError(f"Round 1 前轮问题核销正文必须只有『无。』: {path}")
        return None
    lines = [line for _, line in outside_fence_lines(body, path) if line.strip()]
    summaries = [line for line in lines if SUMMARY_RE.fullmatch(line)]
    if len(summaries) != 1:
        raise ReviewArtifactError(f"Round 2+ 前轮问题核销必须有且仅有一条总结: {path}")
    if lines[-1] != summaries[0]:
        raise ReviewArtifactError(f"Round 2+ 前轮问题核销总结必须位于核销表末尾: {path}")
    table_lines = lines[:-1]
    if len(table_lines) < 2:
        raise ReviewArtifactError(f"Round 2+ 前轮问题核销缺少完整核销表: {path}")
    header = _table_cells(table_lines[0])
    separator = _table_cells(table_lines[1])
    if header != PREVIOUS_TABLE_HEADER or separator != PREVIOUS_TABLE_SEPARATOR:
        raise ReviewArtifactError(f"Round 2+ 前轮问题核销表头或分隔行非法: {path}")
    data_lines = table_lines[2:]
    if not data_lines and summaries[0] != EMPTY_PREVIOUS_SUMMARY:
        raise ReviewArtifactError(f"Round 2+ 空前轮核销表必须使用固定空总结: {path}")
    if data_lines and summaries[0] == EMPTY_PREVIOUS_SUMMARY:
        raise ReviewArtifactError(f"Round 2+ 非空前轮核销表不得使用固定空总结: {path}")
    seen_numbers: set[int] = set()
    seen_sources: set[str] = set()
    for line in data_lines:
        cells = _table_cells(line)
        if cells is None or len(cells) != len(PREVIOUS_TABLE_HEADER):
            raise ReviewArtifactError(f"Round 2+ 前轮问题核销数据行列数非法: {path}")
        number, source, issue, status, evidence = cells
        if not number.isdigit() or int(number) < 1 or int(number) in seen_numbers:
            raise ReviewArtifactError(f"Round 2+ 前轮问题核销序号非法或重复: {path}")
        if not source or source in seen_sources:
            raise ReviewArtifactError(f"Round 2+ 前轮问题核销来源为空或重复: {path}")
        if not issue or not evidence:
            raise ReviewArtifactError(f"Round 2+ 前轮问题核销问题或证据为空: {path}")
        if status not in PREVIOUS_STATUSES:
            raise ReviewArtifactError(f"Round 2+ 前轮问题核销状态非法: {path}")
        seen_numbers.add(int(number))
        seen_sources.add(source)
    return summaries[0]


def _table_cells(line: str) -> tuple[str, ...] | None:
    stripped = line.strip()
    if not stripped.startswith("|") or not stripped.endswith("|"):
        return None
    cells: list[str] = []
    current: list[str] = []
    preceding_backslashes = 0
    for char in stripped[1:-1]:
        if char == "|" and preceding_backslashes % 2 == 0:
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(char)
        preceding_backslashes = preceding_backslashes + 1 if char == "\\" else 0
    cells.append("".join(current).strip())
    return tuple(cells)


def _finding_ids(body: str, path: Path) -> tuple[str, ...]:
    ids: list[str] = []
    for _, line in outside_fence_lines(body, path):
        match = FINDING_ID_RE.match(line)
        if match is not None:
            ids.append(match.group("id"))
    return tuple(ids)


def parse_review_artifact(path: Path) -> ReviewArtifact:
    resolved = path.expanduser().resolve()
    try:
        text = resolved.read_text()
    except OSError as error:
        raise ReviewArtifactError(f"无法读取 review file: {resolved}") from error
    lines = outside_fence_lines(text, resolved)
    mode, round_number = _parse_title(lines, resolved)
    _validate_path_round(resolved, round_number)
    fields = _parse_header_fields(text, lines, resolved)
    target, lane = _target_and_lane(mode, fields, round_number, resolved)
    headings = _headings(text, resolved)
    for fixed_heading in (
        SUBSTANTIVE_HEADING.removeprefix("## "),
        SYNC_HEADING.removeprefix("## "),
    ):
        if sum(heading.title == fixed_heading for heading in headings) != 1:
            raise ReviewArtifactError(f"固定 section『{fixed_heading}』必须恰好一次: {resolved}")
    actual = tuple(heading.title for heading in headings)
    expected = EXPECTED_H2_TITLES[mode]
    if actual != expected:
        raise ReviewArtifactError(
            f"{mode} review 固定 section 不完整或顺序非法: {resolved}；expected={expected}, actual={actual}"
        )

    sections = {heading.title: heading for heading in headings}
    previous_summary = _validate_previous_section(
        text,
        sections[PREVIOUS_HEADING_TITLE],
        resolved,
        round_number,
    )
    substantive = _body(text, sections[SUBSTANTIVE_HEADING.removeprefix("## ")], resolved)
    sync = _body(text, sections[SYNC_HEADING.removeprefix("## ")], resolved)
    try:
        verdict_body = _body(text, headings[-1], resolved)
    except ReviewArtifactError as error:
        raise ReviewArtifactError(f"{mode} review 缺少唯一合法最终判定: {resolved}") from error
    verdicts = [
        match.group("verdict")
        for _, line in outside_fence_lines(verdict_body, resolved)
        if (match := VERDICT_RE.fullmatch(line)) is not None
    ]
    if len(verdicts) != 1 or verdicts[0] not in LEGAL_VERDICTS[mode]:
        raise ReviewArtifactError(f"{mode} review 缺少唯一合法最终判定: {resolved}")
    return ReviewArtifact(
        path=resolved,
        mode=mode,
        round_number=round_number,
        lane=lane,
        target=target,
        substantive=substantive,
        sync=sync,
        finding_ids=_finding_ids(substantive, resolved),
        previous_summary=previous_summary,
        verdict=verdicts[0],
        text=text,
        headings=headings,
    )


def validate_compatible(artifacts: Sequence[ReviewArtifact]) -> None:
    if not artifacts:
        raise ReviewArtifactError("必须提供至少一个 --review-file")
    first = artifacts[0]
    seen_lanes: dict[str, ReviewArtifact] = {}
    labels = {
        "branch": "branch",
        "base": "base",
        "plan": "plan",
        "locked_goal": "locked goal",
    }
    for artifact in artifacts:
        if artifact.mode != first.mode:
            raise ReviewArtifactError(f"review mode conflict: {first.mode} != {artifact.mode}")
        for key in first.target:
            expected = first.target[key]
            actual = artifact.target.get(key)
            # `locked_goal` is free text, and each lane's line breaks and list markers necessarily
            # differ; comparison is relaxed to the normalized form, while the error still prints the
            # original text so the developer is not left hunting for differences between two passages
            # that look identical.
            if key == "locked_goal" and actual is not None:
                if _normalized_for_comparison(actual) == _normalized_for_comparison(expected):
                    continue
            elif actual == expected:
                continue
            raise ReviewArtifactError(f"review {labels.get(key, key)} conflict: {expected} != {actual}")
        previous = seen_lanes.get(artifact.lane)
        if previous is not None:
            raise ReviewArtifactError(
                "同一 lane 出现多个 review file，round 选择有歧义: "
                f"{artifact.lane} (round {previous.round_number}, "
                f"round {artifact.round_number})"
            )
        seen_lanes[artifact.lane] = artifact


def render_chat_response(artifact: ReviewArtifact) -> str:
    parts = [artifact.text[: artifact.headings[0].start]]
    for heading in artifact.headings:
        if heading.title == EXPLORATION_HEADING_TITLE:
            continue
        if heading.title == PREVIOUS_HEADING_TITLE:
            body = artifact.previous_summary or "无。"
            parts.append(artifact.text[heading.start : heading.body_start] + "\n" + body + "\n\n")
            continue
        parts.append(artifact.text[heading.start : heading.end])
    return "".join(parts)
