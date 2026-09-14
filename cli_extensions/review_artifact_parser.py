#!/usr/bin/env python3
"""review.md 的 canonical parser、结构校验与 deterministic renderer。"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Mapping, Sequence


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from review_artifact_task import parse_task_recovery
from review_artifact_types import (
    EMPTY_PREVIOUS_SUMMARY,
    EXPECTED_H2_TITLES,
    EXPLORATION_HEADING_TITLE,
    FENCE_RE,
    FIELD_RE,
    FINDING_ID_RE,
    H2_RE,
    LEGAL_VERDICTS,
    PREVIOUS_HEADING_TITLE,
    PREVIOUS_STATUSES,
    PREVIOUS_TABLE_HEADER,
    PREVIOUS_TABLE_SEPARATOR,
    ROUND_PATH_RE,
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


def _fence_scan(text: str, path: Path | None) -> list[tuple[int, int, str]]:
    """按围栏状态遍历全文，返回围栏外每一行的 `(行号, 起始偏移, 原始行)`。

    围栏口径只此一份：行枚举取行文本、标题定位取字节偏移，取值不同但「哪些行在围栏外」
    必须是同一个判断。两份状态机一旦漂开，同一段正文会得到互相矛盾的结构结论。
    """
    result: list[tuple[int, int, str]] = []
    fence_character: str | None = None
    fence_length = 0
    offset = 0
    for index, line in enumerate(text.splitlines(keepends=True)):
        match = FENCE_RE.match(line)
        if match is not None:
            marker = match.group("fence")
            if fence_character is None:
                fence_character = marker[0]
                fence_length = len(marker)
            elif marker[0] == fence_character and len(marker) >= fence_length:
                fence_character = None
                fence_length = 0
        elif fence_character is None:
            result.append((index, offset, line))
        offset += len(line)
    if fence_character is not None:
        suffix = f": {path}" if path is not None else ""
        raise ReviewArtifactError(f"review file 含未闭合 fenced code block{suffix}")
    return result


def outside_fence_lines(text: str, path: Path | None = None) -> list[tuple[int, str]]:
    return [(index, line.rstrip("\r\n")) for index, _, line in _fence_scan(text, path)]


def _parse_title(lines: list[tuple[int, str]], path: Path) -> tuple[ReviewMode, int]:
    first_nonempty = next((line for _, line in lines if line.strip()), "")
    match = TITLE_RE.fullmatch(first_nonempty)
    if match is None:
        raise ReviewArtifactError(f"无法识别 review mode/title: {path}")
    label_to_mode: Mapping[str, ReviewMode] = {
        "计划审查": "plan",
        "代码审查": "pr",
        "任务验收": "task",
    }
    return label_to_mode[match.group("label")], int(match.group("round"))


def _parse_header_fields(
    text: str, lines: list[tuple[int, str]], path: Path
) -> dict[str, str]:
    """
    解析 header 的 `**键**：值`。**值可以换行续写**：一个 field 一直延续到下一个 field 行、
    第一个 H2，或 header 结束为止。

    早先这里对不匹配 `FIELD_RE` 的行一律 `continue`，于是被折行的值会被**静默截断成第一行**，
    没有任何报错。实测代价：两条 review lane 写同一份「锁定目标」，一条写成一整行、另一条按
    正常 Markdown 折行并带项目符号列表，后者被截掉 14 行只剩开头半句，再与前者逐字比较，
    最终以一条看不出真因的 `locked goal conflict` 停机——而真正的问题是锁定目标（GOAL & SCOPE
    GATE 的权威）本来就已经被悄悄削掉了大半，单 lane 运行同样中招，只是不会报错。
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
        # 结构边界只取围栏外行，值从原文截取，避免丢掉目标中的 fenced 要求。
        flush(index)
        key = match.group("key").strip()
        if key in fields:
            raise ReviewArtifactError(f"header field 重复: {key} ({path})")
        value = match.group("value")
        start = index
    flush(len(raw_lines))
    return fields


_LIST_MARKER_RE = re.compile(r"^[ \t]*[-*+][ \t]+", re.MULTILINE)
# 中日韩表意文字与假名。这类字符之间的折行不是词边界。
_CJK = "\u3040-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff"
# 全角标点：。、；：！？（）「」『』等。它们自带排版留白，紧邻的空白一律不承载信息。
_CJK_PUNCT = "\u3000-\u303f\uff01-\uff60\uffe0-\uffe6"
_CJK_ANY = _CJK + _CJK_PUNCT
_CJK_LINE_JOIN_RE = re.compile(
    rf"(?<=[{_CJK_ANY}])\s+(?=[{_CJK_ANY}])|(?<=[{_CJK_PUNCT}])\s+|\s+(?=[{_CJK_PUNCT}])"
)
_WHITESPACE_RE = re.compile(r"\s+")
# 反引号跨度按 CommonMark 的 code span 规则识别：开头 N 个反引号，结束于**恰好 N 个**的那一段。
# 两处 lookaround 缺一不可：`(?!`)` 挡住「收尾落在更长 run 的前半截」，`(?<!`)` 挡住后半截。
# 少了后者，1 个反引号定界的跨度遇到内部的 ``（CommonMark 允许：N 个反引号的跨度可以包含
# 长度不等于 N 的 run）就会在那里提前收尾，剩下的半截落回散文、其中有意义的空格被抹掉。
_BACKTICK_SPAN_RE = re.compile(r"(?<!`)(`+)(?s:.*?)(?<!`)\1(?!`)")


def _normalized_for_comparison(value: str) -> str:
    """
    跨 lane 比较 header 值时用的宽松形态：抹掉行首列表符与全部空白差异。

    只用于**比较**，不改变 `target` 里存下来、后续渲染与裁决要引用的原文。放宽的边界刻意很窄：
    两条 lane 是各自把同一份锁定目标重新序列化的，折行位置与列表符号必然不同，但用词与顺序不会；
    真正写了不同目标的两条 lane 仍然会被拦下。

    CJK 之间的折行必须收成**空字符串**而不是一个空格：中文没有词间空格，`产出出口：` 与
    `「导出时间线项目」` 之间那个换行在另一条 lane 的单行写法里根本不存在。全角标点紧邻的空白
    同理——一条 lane 用项目符号列表分段、另一条把同样的句子直接接在 `。` 后面，差的只是排版。
    一律换成空格会在本次的两条 lane 之间凭空造出 14 处差异，实测正是卡在这里。

    仍然保留的是 CJK 与拉丁文之间的空格（`的 spine`）：那是真实的分词位置，两条 lane 都会写。

    **反引号跨度整段豁免**：里面是字面量——文件名、标识符、命令——空白在那里是有意义的，不是排版。
    对整段无差别归一化会让 `` `成 片.fcpxml` `` 与 `` `成片.fcpxml` `` 被判等价，于是「两条 lane
    锁定了不同产物名」这种真冲突被静默放行。散文照常放宽，字面量逐字比较。

    紧邻字面量的空白仍按排版处理（见下方实现）：`` 集 `X` ``、`` 集`X` `` 与断在此处折行三者等价。
    真正保留的只有字面量**内部**的空白。残留的保守面是折行断在跨度内部——那会被判冲突；误报当场
    停机、看得见也改得掉，漏报则是静默接受两份不同的目标，而锁定目标正是 GOAL & SCOPE GATE 的权威。
    """
    segments = _split_literals(value)
    rendered: list[str] = []
    for index, (is_literal, text) in enumerate(segments):
        if is_literal:
            rendered.append(text)
            continue
        prose = _normalize_prose(text)
        # 紧邻字面量的空白只是排版：同一句话在另一条 lane 里可能写成 `` 集 `X` ``、`` 集`X` `` 或
        # 断在这里折行。三种写法必须归一到同一形态。这一步不能交给 `_CJK_LINE_JOIN_RE`——它的
        # lookaround 需要换行两侧的字符，而切片边界那一侧的字符属于相邻片段，匹配不到。
        if index > 0:
            prose = prose.lstrip()
        if index < len(segments) - 1:
            prose = prose.rstrip()
        rendered.append(prose)
    return "".join(rendered).strip()


def _split_literals(value: str) -> list[tuple[bool, str]]:
    # 与结构解析共用围栏边界；围栏正文也不能经过散文空白归一化。
    segments: list[tuple[bool, str]] = []
    prose: list[str] = []
    cursor = 0
    for _, offset, line in _fence_scan(value, None):
        if cursor < offset:
            segments.extend(_split_inline_literals("".join(prose)))
            prose.clear()
            segments.append((True, value[cursor:offset]))
        prose.append(line)
        cursor = offset + len(line)
    segments.extend(_split_inline_literals("".join(prose)))
    if cursor < len(value):
        segments.append((True, value[cursor:]))
    return segments


def _split_inline_literals(value: str) -> list[tuple[bool, str]]:
    segments: list[tuple[bool, str]] = []
    cursor = 0
    for match in _BACKTICK_SPAN_RE.finditer(value):
        segments.append((False, value[cursor : match.start()]))
        segments.append((True, match.group(0)))
        cursor = match.end()
    segments.append((False, value[cursor:]))
    return segments


def _normalize_prose(part: str) -> str:
    joined = _CJK_LINE_JOIN_RE.sub("", _LIST_MARKER_RE.sub("", part))
    return _WHITESPACE_RE.sub(" ", joined)


def _require_field(fields: Mapping[str, str], key: str, path: Path) -> str:
    value = fields.get(key)
    if not value:
        raise ReviewArtifactError(f"缺少或为空的 header field: {key} ({path})")
    return value


def _round_from_output_lane(output_lane: str, path: Path) -> tuple[int, str]:
    normalized = output_lane.replace("\\", "/").rstrip("/")
    match = ROUND_PATH_RE.search(normalized)
    if match is None:
        raise ReviewArtifactError(f"task 输出 lane 缺少明确 round-NN/review.md: {path}")
    round_number = int(match.group("round"))
    lane = normalized[: match.start()].rstrip("/")
    if not lane:
        raise ReviewArtifactError(f"task 输出 lane identity 为空: {path}")
    return round_number, lane


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
    output_lane = _require_field(fields, "输出 lane", path)
    output_round, lane = _round_from_output_lane(output_lane, path)
    if output_round != title_round:
        raise ReviewArtifactError(f"task lane round 与 title round 不一致: {path}")
    scope_hash = _require_field(fields, "SCOPE_HASH", path)
    if not re.fullmatch(r"[0-9a-f]{64}", scope_hash):
        raise ReviewArtifactError(f"SCOPE_HASH 格式不合法: {path}")
    return {
        "plan": _require_field(fields, "计划", path),
        "task": _require_field(fields, "任务", path),
        "scope_hash": scope_hash,
    }, lane


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
    recovery_reason, producer_task_id = parse_task_recovery(
        mode,
        verdicts[0],
        outside_fence_lines(verdict_body, resolved),
        resolved,
    )
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
        recovery_reason=recovery_reason,
        producer_task_id=producer_task_id,
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
        "task": "task",
        "scope_hash": "SCOPE_HASH",
    }
    for artifact in artifacts:
        if artifact.mode != first.mode:
            raise ReviewArtifactError(f"review mode conflict: {first.mode} != {artifact.mode}")
        for key in first.target:
            expected = first.target[key]
            actual = artifact.target.get(key)
            # `locked_goal` 是自由文本，各 lane 的折行与列表符号必然不同；比较放宽到规范化形态，
            # 报错仍然照原文打印，免得开发者对着两段看起来一样的文字找不同。
            if key == "locked_goal" and actual is not None:
                if _normalized_for_comparison(actual) == _normalized_for_comparison(expected):
                    continue
            elif actual == expected:
                continue
            raise ReviewArtifactError(
                f"review {labels.get(key, key)} conflict: {expected} != {actual}"
            )
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
