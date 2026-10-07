#!/usr/bin/env python3
"""Fence scanning for review.md and text normalization for comparing header values across lanes."""

from __future__ import annotations

import re
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from review_artifact_types import FENCE_RE, ReviewArtifactError


def _fence_scan(text: str, path: Path | None) -> list[tuple[int, int, str]]:
    """Walk the whole text by fence state and return `(line number, start offset, raw line)` for every line outside a fence.

    There is exactly one fence rule: line enumeration takes the line text and heading lookup takes
    the byte offset, but "which lines are outside a fence" must be the same judgment. Once two state
    machines drift apart, the same body yields contradictory structural conclusions.
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


_LIST_MARKER_RE = re.compile(r"^[ \t]*[-*+][ \t]+", re.MULTILINE)
# CJK ideographs and kana. A line break between these characters is not a word boundary.
_CJK = "぀-ヿ㐀-䶿一-鿿豈-﫿"
# Full-width punctuation: 。、；：！？（）「」『』 etc. They carry their own typographic spacing, so
# adjacent whitespace never carries information.
_CJK_PUNCT = "　-〿！-｠￠-￦"
_CJK_ANY = _CJK + _CJK_PUNCT
_CJK_LINE_JOIN_RE = re.compile(rf"(?<=[{_CJK_ANY}])\s+(?=[{_CJK_ANY}])|(?<=[{_CJK_PUNCT}])\s+|\s+(?=[{_CJK_PUNCT}])")
_WHITESPACE_RE = re.compile(r"\s+")
# Backtick spans follow CommonMark's code span rule: N opening backticks, closed by the run of
# **exactly N**. Both lookarounds are required: `(?!`)` stops "closing on the front half of a longer
# run", `(?<!`)` stops the back half. Without the latter, a span delimited by 1 backtick that contains
# an inner `` (CommonMark allows it: a span of N backticks may contain runs whose length is not N)
# closes early there, and the remaining half falls back into prose where its meaningful spaces are erased.
_BACKTICK_SPAN_RE = re.compile(r"(?<!`)(`+)(?s:.*?)(?<!`)\1(?!`)")


def _normalized_for_comparison(value: str) -> str:
    """
    The relaxed form used when comparing header values across lanes: erase leading list markers and all whitespace differences.

    Used only for **comparison**; it does not change the original text stored in `target` that later
    rendering and triage cite. The relaxation is deliberately narrow: two lanes each re-serialize the
    same locked goal, so their line breaks and list markers necessarily differ, but their wording and
    order do not; two lanes that really wrote different goals are still stopped.

    A line break between CJK characters must collapse to the **empty string**, not a space: Chinese has
    no inter-word spaces, and the newline between `产出出口：` and `「导出时间线项目」` simply does not
    exist in the other lane's single-line form. Whitespace next to full-width punctuation likewise — one
    lane splits paragraphs with a bullet list, the other appends the same sentence right after `。`; the
    only difference is layout. Turning all of it into a space conjured 14 differences between the two
    lanes of that run, and that is exactly where it got stuck in practice.

    What is still kept is the space between CJK and Latin text (`的 spine`): that is a real word
    boundary, and both lanes write it.

    **Backtick spans are exempt as a whole**: their content is literal — file names, identifiers,
    commands — and whitespace there is meaningful, not layout. Normalizing indiscriminately would make
    `` `成 片.fcpxml` `` and `` `成片.fcpxml` `` compare equal, silently letting through a real conflict
    such as "the two lanes locked different artifact names". Prose stays relaxed; literals are compared
    verbatim.

    Whitespace adjacent to a literal is still treated as layout (see the implementation below):
    `` 集 `X` ``, `` 集`X` `` and a line break at this point are equivalent. Only the whitespace
    **inside** a literal is preserved. The remaining conservative edge is a line break inside a span —
    that is judged a conflict; a false positive stops on the spot, visibly and fixably, whereas a false
    negative silently accepts two different goals, and the locked goal is precisely the authority of
    the GOAL & SCOPE GATE.
    """
    segments = _split_literals(value)
    rendered: list[str] = []
    for index, (is_literal, text) in enumerate(segments):
        if is_literal:
            rendered.append(text)
            continue
        prose = _normalize_prose(text)
        # Whitespace next to a literal is only layout: the same sentence may be written in another
        # lane as `` 集 `X` ``, `` 集`X` `` or broken across lines here. All three must normalize to the
        # same form. This step cannot be left to `_CJK_LINE_JOIN_RE` — its lookarounds need characters
        # on both sides of the break, and the character on the slice-boundary side belongs to the
        # adjacent segment, so it cannot match.
        if index > 0:
            prose = prose.lstrip()
        if index < len(segments) - 1:
            prose = prose.rstrip()
        rendered.append(prose)
    return "".join(rendered).strip()


def _split_literals(value: str) -> list[tuple[bool, str]]:
    # Shares fence boundaries with structural parsing; fenced bodies must not go through prose
    # whitespace normalization either.
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
