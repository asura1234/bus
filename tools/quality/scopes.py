"""Shared test paths and balanced Rust cfg scopes; no policy activation here."""
from __future__ import annotations

import fnmatch
import itertools
import re
from dataclasses import dataclass
from pathlib import PurePath

from tools.quality.rust_source import blank_non_newlines, mask_comments_and_literals


DEFAULT_TEST_FILES = ("*_test.*",)
DEFAULT_TEST_DIRS = ("tests",)
ATTRIBUTE = re.compile(r"#\s*(!?)\s*\[")
CFG_TOKEN = re.compile(r'\w+|"(?:\\.|[^"\\])*"|[(),=]')
TEST_ATTRIBUTE = re.compile(r"^(?:\w+\s*::\s*)*(?:test|should_panic)\b")


def is_test_path(path: str | PurePath, *, test_files=DEFAULT_TEST_FILES,
                 test_dirs=DEFAULT_TEST_DIRS) -> bool:
    path = PurePath(str(path).replace("\\", "/"))
    return any(part in test_dirs for part in path.parts[:-1]) or any(
        fnmatch.fnmatchcase(path.name, pattern) for pattern in test_files
    )


def _split_arguments(text: str) -> list[str]:
    """Split only top-level cfg commas; literals remain atomic tokens."""
    pieces = []
    start = depth = 0
    for match in CFG_TOKEN.finditer(text):
        if match[0] == "(":
            depth += 1
        elif match[0] == ")":
            depth -= 1
        elif match[0] == "," and depth == 0:
            pieces.append(text[start:match.start()].strip())
            start = match.end()
    if tail := text[start:].strip():
        pieces.append(tail)
    return pieces


def _cfg_expression(text: str):
    text = text.strip()
    call = re.fullmatch(r"(all|any|not)\s*\((.*)\)", text, re.S)
    if call:
        name, body = call.groups()
        children = tuple(_cfg_expression(arg) for arg in _split_arguments(body))
        if name == "not" and len(children) != 1:
            raise ValueError(f"invalid cfg expression: {text}")
        return name, children
    # Each platform/feature predicate is one independent boolean atom.
    if not re.fullmatch(r'\w+(?:\s*=\s*"(?:\\.|[^"\\])*")?', text):
        raise ValueError(f"invalid cfg expression: {text}")
    return "atom", re.sub(r"\s+", "", text)


def _atoms(expression) -> set[str]:
    kind, value = expression
    if kind == "atom":
        return {value}
    return set().union(*(_atoms(child) for child in value))


def _evaluate(expression, values: dict[str, bool]) -> bool:
    kind, value = expression
    if kind == "atom":
        return values[value]
    if kind == "not":
        return not _evaluate(value[0], values)
    return (all if kind == "all" else any)(_evaluate(child, values) for child in value)


def cfg_requires_test(expression: str) -> bool:
    """True only when this condition cannot hold with cfg(test) disabled."""
    return _requires_test(_cfg_expression(expression))


def _requires_test(expression) -> bool:
    atoms = sorted(_atoms(expression) - {"test"})
    # Fail conservatively on unexpectedly enormous generated predicates.
    if len(atoms) > 12:
        return False
    return not any(
        _evaluate(expression, {"test": False, **dict(zip(atoms, values))})
        for values in itertools.product((False, True), repeat=len(atoms))
    )


def _attribute_condition(attribute: str):
    call = re.fullmatch(r"(cfg|cfg_attr)\s*\((.*)\)", attribute.strip(), re.S)
    if not call:
        return None
    kind, body = call.groups()
    if kind == "cfg":
        return _cfg_expression(body)
    arguments = _split_arguments(body)
    if len(arguments) < 2:
        raise ValueError("cfg_attr requires a predicate and an attribute")
    conditions = tuple(
        condition for argument in arguments[1:]
        if (condition := _attribute_condition(argument)) is not None
    )
    if not conditions:
        return None
    # cfg_attr(P, cfg(Q)) admits the item exactly when !P || Q.
    return "any", (("not", (_cfg_expression(arguments[0]),)), ("all", conditions))


def balanced_pairs(code: str) -> dict[int, int]:
    stack = []
    pairs = {}
    for index, char in enumerate(code):
        if char in "([{":
            stack.append((char, index))
        elif char in ")]}":
            if stack and stack[-1][0] == {")": "(", "]": "[", "}": "{"}[char]:
                _, start = stack.pop()
                pairs[start] = index
    return pairs


@dataclass(frozen=True)
class RustItem:
    start: int
    header: int
    end: int
    attributes: tuple[str, ...]
    inner: bool = False

    @property
    def requires_test(self) -> bool:
        conditions = tuple(
            condition for attribute in self.attributes
            if (condition := _attribute_condition(attribute)) is not None
        )
        return bool(conditions) and _requires_test(("all", conditions))

    @property
    def is_test_body(self) -> bool:
        return any(TEST_ATTRIBUTE.match(attribute.strip()) for attribute in self.attributes)


def _arm_arrow(code: str, start: int, pairs: dict[int, int]) -> int | None:
    if re.match(r"(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+|async\s+)?(?:fn|impl|mod|struct|enum|trait|use|const|static|type|extern)\b", code[start:]):
        return None
    index = start
    while index < len(code):
        if code.startswith("=>", index):
            return index
        if code[index] in "([{":
            index = pairs.get(index, len(code) - 1) + 1
        elif code[index] in ",;}":
            return None
        else:
            index += 1
    return None


def _item_end(code: str, start: int, pairs: dict[int, int]) -> int:
    index = start
    angles = 0
    arrow = _arm_arrow(code, start, pairs)
    expression = arrow is not None
    while index < len(code):
        char = code[index]
        if char in "([":
            index = pairs.get(index, len(code) - 1) + 1
            continue
        if char == "=":
            expression = True
        if char == "<" and (angles or not expression or code[max(0, index - 2):index] == "::"):
            angles += 1
        elif char == ">" and not code[max(0, index - 1):index] in {"=", "-"}:
            angles = max(0, angles - 1)
        elif char == "{" and (angles or (arrow is not None and index < arrow)):
            index = pairs.get(index, len(code) - 1) + 1
            continue
        elif char == "{":
            end = pairs.get(index, len(code) - 1) + 1
            tail = end
            while tail < len(code) and code[tail].isspace():
                tail += 1
            return tail + 1 if code[tail:tail + 1] in {";", ","} else end
        elif char in ";," and angles == 0:
            return index + 1
        elif char == "}":
            return index
        index += 1
    return len(code)


def rust_items(source: str) -> tuple[RustItem, ...]:
    code = mask_comments_and_literals(source)
    pairs = balanced_pairs(code)
    items = []
    consumed = 0
    for match in ATTRIBUTE.finditer(code):
        if match.start() < consumed:
            continue
        start = match.start()
        inner = bool(match[1])
        attributes = []
        current = match
        while current:
            close = pairs.get(current.end() - 1)
            if close is None:
                raise ValueError("unbalanced Rust attribute")
            # Keep original literal values for cfg and path attributes, but remove comments.
            raw = source[current.end():close]
            attributes.append(mask_comments_and_literals(raw, mask_literals=False).strip())
            consumed = close + 1
            header = consumed
            while header < len(code) and code[header].isspace():
                header += 1
            current = ATTRIBUTE.match(code, header)
            if current and bool(current[1]) != inner:
                current = None
        if inner:
            containers = [(left, right) for left, right in pairs.items()
                          if code[left] == "{" and left < start < right]
            left, right = max(containers, default=(-1, len(code)), key=lambda pair: pair[0])
            items.append(RustItem(left + 1, header, right, tuple(attributes), True))
        else:
            items.append(RustItem(start, header, _item_end(code, header, pairs), tuple(attributes)))
    return tuple(items)


def merge_ranges(ranges) -> tuple[tuple[int, int], ...]:
    merged = []
    for start, end in sorted(ranges):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    return tuple(merged)


def rust_test_ranges(source: str) -> tuple[tuple[int, int], ...]:
    return merge_ranges((item.start, item.end) for item in rust_items(source) if item.requires_test)


def in_ranges(offset: int, ranges) -> bool:
    return any(start <= offset < end for start, end in ranges)


def production_code(source: str) -> str:
    code = mask_comments_and_literals(source)
    chars = list(code)
    for start, end in rust_test_ranges(source):
        blank_non_newlines(chars, start, end)
    return "".join(chars)


def production_line_count(source: str, path: str | PurePath = "") -> int:
    """Physical lines, including comments/blanks outside test-only item ranges."""
    if is_test_path(path):
        return 0
    from bisect import bisect_right

    chars = list(source)
    starts = [0, *(match.end() for match in re.finditer("\n", source))]
    covered = set()
    for start, end in rust_test_ranges(source):
        covered.update(range(bisect_right(starts, start) - 1, bisect_right(starts, max(start, end - 1))))
        blank_non_newlines(chars, start, end)
    # A line shared with any neighboring production bytes still counts.
    lines = "".join(chars).splitlines()
    return len(lines) - sum(not lines[line].strip() for line in covered if line < len(lines))
