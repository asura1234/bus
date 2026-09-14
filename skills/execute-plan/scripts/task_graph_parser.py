#!/usr/bin/env python3
"""解析计划任务块与字段，不承担任务图或 owner 校验。"""

from __future__ import annotations

import re
from dataclasses import dataclass, field


REQUIRED_TASK_FIELDS: frozenset[str] = frozenset(
    {
        "完成",
        "目标",
        "拥有文件",
        "blocked-by",
        "produces",
        "consumes",
        "工具",
        "约束",
        "验收闸门",
    }
)

_VALUE_FIELDS = REQUIRED_TASK_FIELDS - {"完成"}
_OLD_TOKEN_RE = re.compile(
    r"(?:^\s*(?:#{1,6}\s*)?\*{0,2}阶段\s*\d+\*{0,2}\s*[:：])"
    r"|(?:^\s*-\s*\[[ xX]\]\s*\*{0,2}步骤\s*\d+\*{0,2}\s*[:：])"
)
_MANUAL_MARKER = "(需要手动操作)"
_TASK_HEADER_RE = re.compile(r"^任务\s*(\d+)\s*[:：]\s*(.*)$")
_FIELD_RE = re.compile(r"^\s*-\s+\*\*([^*]+)\*\*\s*([:：])\s*(.*)$")
_BOLD_FIELD_CANDIDATE_RE = re.compile(r"^\s*-\s+\*\*([^*]+)\*\*(.*)$")
_DONE_RE = re.compile(r"^\s*-\s*\[([ xX])\]\s*\*\*完成\*\*\s*$")
_TASK_REFERENCE_RE = re.compile(r"^任务\s*(\d+)$")
_GLOB_CHARS_RE = re.compile(r"[*?\[]")
CREATED_DATE_RE = re.compile(r"^\*\*创建日期\*\*[:：]\s*(\d{4}-\d{2}-\d{2})", re.MULTILINE)
CREATED_DATE_LABEL_RE = re.compile(r"^\*\*创建日期\*\*[:：]", re.MULTILINE)
_DECLARED_PATH_OWNER_REQUIRED_SINCE = "2026-07-23"
_GATE_LEVEL_REQUIRED_SINCE = "2026-07-24"
# Canonical plans use plain file-contract bullets. The optional checkbox keeps
# in-flight plans created from older templates executable without treating the
# checkbox state as review evidence.
_FILE_CONTRACT_ENTRY_RE = re.compile(
    r"^\s*-\s*(?:\[[ xX]\]\s*)?\*\*[^*]*文件\*\*\s*[:：]\s*(.+?)\s*$"
)
_TEST_FILE_HEADING_RE = re.compile(r"^\s*#{3,6}\s+\*{0,2}测试文件\*{0,2}\s*[:：]\s*(.+?)\s*$")
_BACKTICK_VALUE_RE = re.compile(r"`([^`\n]+)`")
_GATE_LEVEL_RE = re.compile(r"^\[(?P<level>[A-Z_]+)\]\s+(?P<body>.+)$", re.DOTALL)


@dataclass(frozen=True)
class Task:
    """任务图算法所需的最小任务契约。"""

    id: int
    name: str
    owned_files: tuple[str, ...]
    blocked_by: tuple[int, ...]
    consumes: tuple[int, ...]
    done: bool
    gate_level: str | None = None
    gate_commands: tuple[str, ...] = ()
    line: int = field(default=0, repr=False, compare=False)


@dataclass(frozen=True)
class _ScannedLine:
    number: int
    text: str
    raw: str
    fenced: bool


@dataclass
class _TaskBlock:
    task_id: int
    name: str
    line: int
    header: _ScannedLine
    lines: list[_ScannedLine]

    @property
    def text(self) -> str:
        return self.header.raw + "".join(line.raw for line in self.lines)


@dataclass
class _ParsedBlock:
    task: Task | None
    errors: list[str]


@dataclass(frozen=True)
class ParsedTaskBlock:
    """权威任务解析结果及其逐字源码块。"""

    task: Task
    text: str


def fence_marker(line: str) -> tuple[str, int] | None:
    match = re.match(r"^\s*(`{3,}|~{3,})", line)
    if not match:
        return None
    marker = match.group(1)
    return marker[0], len(marker)


def _scan_section(plan_text: str, heading: str) -> tuple[list[_ScannedLine], bool, list[str]]:
    """扫描指定 H2 section；只有 fenced code 之外的标题参与边界判定。"""

    lines = plan_text.splitlines(keepends=True)
    result: list[_ScannedLine] = []
    errors: list[str] = []
    in_section = False
    found_section = False
    fence: tuple[str, int] | None = None
    fence_line: int | None = None

    for number, raw in enumerate(lines, start=1):
        text = raw.rstrip("\r\n")
        marker = fence_marker(text)
        was_fenced = fence is not None

        if fence is None and marker is not None:
            fence = marker
            fence_line = number
            line_is_fenced = True
        elif fence is not None:
            line_is_fenced = True
            if marker is not None and marker[0] == fence[0] and marker[1] >= fence[1]:
                fence = None
                fence_line = None
        else:
            line_is_fenced = False

        if not in_section:
            if not was_fenced and not line_is_fenced and text.strip() == f"## {heading}":
                in_section = True
                found_section = True
            continue

        if not was_fenced and not line_is_fenced and re.match(r"^##\s+", text):
            break
        result.append(_ScannedLine(number, text, raw, line_is_fenced))

    if fence is not None:
        errors.append(f"行 {fence_line}：fenced code block 未闭合")
    return result, found_section, errors


def extract_section_body(plan_text: str, heading: str) -> str:
    """返回指定 H2 section 的原文内容，fenced heading 不会误切断。"""

    lines, found_section, _ = _scan_section(plan_text, heading)
    if not found_section:
        return ""
    return "\n".join(line.text for line in lines)


def _scan_implementation_section(
    plan_text: str,
) -> tuple[list[_ScannedLine], bool, list[str]]:
    return _scan_section(plan_text, "实施步骤")


def _header_parts(line: _ScannedLine) -> tuple[int, str] | None:
    if line.fenced:
        return None
    match = re.match(r"^\s*###\s+(.+?)\s*$", line.text)
    if not match:
        return None
    inner = match.group(1).strip()
    if inner.startswith("**") and inner.endswith("**") and len(inner) >= 4:
        inner = inner[2:-2].strip()
    task_match = _TASK_HEADER_RE.fullmatch(inner)
    if not task_match:
        return None
    return int(task_match.group(1)), task_match.group(2).strip()


def _is_task_header_candidate(line: _ScannedLine) -> bool:
    if line.fenced:
        return False
    return bool(re.match(r"^\s*###\s+\*{0,2}任务\s*\d+", line.text))


def _collect_blocks(lines: list[_ScannedLine]) -> tuple[list[_TaskBlock], list[str]]:
    blocks: list[_TaskBlock] = []
    errors: list[str] = []
    current: _TaskBlock | None = None

    for line in lines:
        parts = _header_parts(line)
        if parts is not None:
            if current is not None:
                blocks.append(current)
            task_id, name = parts
            current = _TaskBlock(task_id, name, line.number, line, [])
            continue
        if not line.fenced and re.match(r"^\s*###\s+", line.text):
            if current is not None:
                blocks.append(current)
                current = None
            if _is_task_header_candidate(line):
                errors.append(f"行 {line.number}：任务标题格式错误，必须使用冒号分隔 id 与名称")
            continue
        if current is not None:
            current.lines.append(line)

    if current is not None:
        blocks.append(current)
    return blocks, errors


def _split_values(value: str) -> tuple[str, ...]:
    if value.strip() == "无":
        return ()
    parts: list[str] = []
    for raw in re.split(r"[、,，;；\n]+", value):
        item = raw.strip()
        if item.startswith("-"):
            item = item[1:].strip()
        if len(item) >= 2 and item.startswith("`") and item.endswith("`"):
            item = item[1:-1].strip()
        if item:
            parts.append(item)
    return tuple(parts)


def _parse_task_references(value: str, line: int, label: str) -> tuple[tuple[int, ...], list[str]]:
    references: list[int] = []
    errors: list[str] = []
    for item in _split_values(value):
        match = _TASK_REFERENCE_RE.fullmatch(item)
        if not match:
            errors.append(f"行 {line}：{label} 的引用 {item!r} 必须使用任务<N>形态")
            continue
        references.append(int(match.group(1)))
    return tuple(references), errors


def _parse_block(block: _TaskBlock) -> _ParsedBlock:
    errors: list[str] = []
    values: dict[str, tuple[str, int]] = {}
    done: bool | None = None
    current_field: str | None = None

    if not block.name:
        errors.append(f"行 {block.line}：任务名称不能为空")

    for line in block.lines:
        if line.fenced:
            if current_field is not None and current_field in values:
                previous, field_line = values[current_field]
                values[current_field] = (f"{previous}\n{line.text.strip()}".strip(), field_line)
            continue

        done_match = _DONE_RE.fullmatch(line.text)
        if done_match:
            if done is not None:
                errors.append(f"行 {line.number}：完成字段重复")
            done = done_match.group(1).lower() == "x"
            current_field = None
            continue

        field_match = _FIELD_RE.fullmatch(line.text)
        if field_match:
            label = field_match.group(1).strip()
            if label not in _VALUE_FIELDS:
                current_field = None
                continue
            if label in values:
                errors.append(f"行 {line.number}：字段 {label} 重复")
            values[label] = (field_match.group(3).strip(), line.number)
            current_field = label
            continue

        candidate = _BOLD_FIELD_CANDIDATE_RE.fullmatch(line.text)
        if candidate and candidate.group(1).strip() in REQUIRED_TASK_FIELDS:
            label = candidate.group(1).strip()
            errors.append(f"行 {line.number}：字段 {label} 标签缺少冒号")
            current_field = None
            continue

        if "**完成**" in line.text and line.text.lstrip().startswith("-"):
            errors.append(f"行 {line.number}：完成字段必须使用 - [ ] 或 - [x] 复选框")
            current_field = None
            continue

        if current_field is not None and line.text[:1].isspace():
            previous, field_line = values[current_field]
            continuation = line.text.strip()
            if continuation:
                values[current_field] = (f"{previous}\n{continuation}".strip(), field_line)

    if done is None:
        errors.append(f"行 {block.line}：任务缺少字段 完成")

    for label in sorted(_VALUE_FIELDS):
        if label not in values:
            errors.append(f"行 {block.line}：任务缺少字段 {label}")
        elif not values[label][0].strip():
            errors.append(f"行 {values[label][1]}：字段 {label} 不能为空")

    blocked_by: tuple[int, ...] = ()
    consumes: tuple[int, ...] = ()
    if "blocked-by" in values and values["blocked-by"][0].strip():
        blocked_by, reference_errors = _parse_task_references(
            values["blocked-by"][0], values["blocked-by"][1], "blocked-by"
        )
        errors.extend(reference_errors)
    if "consumes" in values and values["consumes"][0].strip():
        consumes, reference_errors = _parse_task_references(values["consumes"][0], values["consumes"][1], "consumes")
        errors.extend(reference_errors)

    if errors:
        return _ParsedBlock(None, errors)

    gate_value = values["验收闸门"][0]
    gate_match = _GATE_LEVEL_RE.fullmatch(gate_value)
    gate_level = gate_match.group("level") if gate_match else None
    gate_body = gate_match.group("body") if gate_match else gate_value
    return _ParsedBlock(
        Task(
            id=block.task_id,
            name=block.name,
            owned_files=_split_values(values["拥有文件"][0]),
            blocked_by=blocked_by,
            consumes=consumes,
            done=bool(done),
            gate_level=gate_level,
            gate_commands=tuple(_BACKTICK_VALUE_RE.findall(gate_body)),
            line=block.line,
        ),
        [],
    )


def _parse_all(
    plan_text: str,
) -> tuple[list[ParsedTaskBlock], list[_TaskBlock], list[str], list[_ScannedLine], bool]:
    section_lines, found_section, scan_errors = _scan_implementation_section(plan_text)
    blocks, errors = _collect_blocks(section_lines)
    errors = scan_errors + errors
    parsed_blocks: list[ParsedTaskBlock] = []
    for block in blocks:
        parsed = _parse_block(block)
        errors.extend(parsed.errors)
        if parsed.task is not None:
            parsed_blocks.append(ParsedTaskBlock(parsed.task, block.text))
    return parsed_blocks, blocks, errors, section_lines, found_section


def parse_task_blocks(plan_text: str) -> list[ParsedTaskBlock]:
    """解析实施步骤中的任务及逐字源码块；所有消费者共享此边界语义。"""

    parsed_blocks, _, errors, _, _ = _parse_all(plan_text)
    if errors:
        raise ValueError("\n".join(errors))
    return parsed_blocks


def parse_tasks(plan_text: str) -> list[Task]:
    """解析实施步骤中的任务块；格式错误时抛出含源码行号的异常。"""

    return [block.task for block in parse_task_blocks(plan_text)]
