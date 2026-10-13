#!/usr/bin/env python3
"""Mechanical layer of the pr skill: check that the PR title and body conform to the PR templates.

Two files are the format source of truth, each for its own part: the title type enum comes from the
`**类型**：` line of references/pr-template.md, and the body sections, placeholders and self-test
checkboxes come from .github/pull_request_template.md, the same body GitHub offers a person opening a
PR, so the two can never drift. This script proves only structure and checkbox state; it does not
judge content quality.

Rules (all fail-closed):
- The title has the shape `[类型] 简短描述`, the type belongs to the template enum, and the
  description is non-empty with no leading/trailing whitespace.
- Body H2 sections match the template (same order), and every section is non-empty.
- The `draft` phase lets `自测 / Agent 测` keep `- [ ]` pending items; other sections forbid unchecked items.
- In the `final` phase, `自测 / Agent 测` has at least one checkbox and all are `- [x]`; the whole body
  forbids unchecked items.
- Every checkbox of the template's `自测 / Agent 测` appears exactly once in the body, by its name; a
  checked one must cite the head SHA and report it rests on.
- The whole body (outside code fences) allows no HTML comments or leftover template placeholders.
- `目标` / `非目标` must equal the mechanically generated context verbatim, and the review-pr lock must equal the goal.
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence

from pr_goal_context import parse_context, split_h2_sections


SELF_TEST_SECTION = "自测 / Agent 测"
GOAL_SECTION = "目标"
NON_GOAL_SECTION = "非目标"
PHASES = ("draft", "final")
# Placeholders are derived from the template body, not hard-coded: a hard-coded list covers only the
# few thought of at the time, placeholders added to the template later slip through silently, and the
# whole point of the gate is to block template residue.
_PLACEHOLDER_PATTERN = re.compile(r"\[([^\[\]\n]+)\]")
_CHECKBOX_PREFIX = re.compile(r"^\s*[-*]\s*\[[ xX]\]\s*")
_CHECKBOX = re.compile(r"^- \[([ x])\] (\S.*)$")
# A required box is ticked only on evidence: the head it proves and the report that proves it.
_BOX_EVIDENCE = re.compile(r"head `[0-9a-f]{7,40}`.*report `[^`]+`")
REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_BODY_TEMPLATE = REPO_ROOT / ".github" / "pull_request_template.md"


def _lines_outside_fences(text: str) -> list[str]:
    lines: list[str] = []
    inside_fence = False
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            inside_fence = not inside_fence
            continue
        if not inside_fence:
            lines.append(line)
    return lines


def _derive_body_placeholders(description_text: str) -> list[str]:
    """Extract placeholders (slots to fill, shaped like `[…]`) from the body template.

    Three kinds of brackets that are not slots are excluded line by line: lines inside code fences,
    `>` instruction blocks and HTML comments (both are instructions to the AI and never reach the final
    PR), and the `- [x]` checkbox prefix plus Markdown links of the form `[text](link)`.
    """
    placeholders: list[str] = []
    for line in _lines_outside_fences(description_text):
        stripped = line.lstrip()
        if stripped.startswith((">", "<!--", "-->")):
            continue
        content = _CHECKBOX_PREFIX.sub("", line)
        for match in _PLACEHOLDER_PATTERN.finditer(content):
            end = match.end()
            if end < len(content) and content[end] == "(":
                continue
            if match.group(0) not in placeholders:
                placeholders.append(match.group(0))
    if not placeholders:
        raise ValueError("template defines no body placeholders")
    return placeholders


def parse_title_types(rules_text: str) -> list[str]:
    """Extract the title type enum from the `**类型**：` line of references/pr-template.md."""
    type_line = next(
        (line for line in rules_text.splitlines() if line.startswith("**类型**：")),
        None,
    )
    if type_line is None:
        raise ValueError("template is missing the `**类型**：` line")
    type_match = re.search(r"`([^`]+)`", type_line)
    if type_match is None:
        raise ValueError("template `**类型**：` line has no backtick enum")
    title_types = [item.strip() for item in type_match.group(1).split("|")]
    if not title_types or any(not item for item in title_types):
        raise ValueError("template title type enum is malformed")
    return title_types


def parse_body_template(body_template_text: str) -> tuple[list[str], list[str], list[str]]:
    """Extract (H2 section list, placeholder list, required self-test box names) from the body template."""
    sections = split_h2_sections(body_template_text)
    if not sections:
        raise ValueError("body template defines no sections")
    self_test = dict(sections).get(SELF_TEST_SECTION)
    if self_test is None:
        raise ValueError(f"body template is missing `## {SELF_TEST_SECTION}`")
    boxes = [
        match.group(2).strip()
        for line in _lines_outside_fences(self_test)
        if (match := _CHECKBOX.match(line.strip())) is not None
    ]
    if not boxes:
        raise ValueError(f"body template `{SELF_TEST_SECTION}` defines no checkboxes")
    return (
        [title for title, _content in sections],
        _derive_body_placeholders(body_template_text),
        boxes,
    )


def check_required_boxes(self_test: str, required_boxes: Sequence[str]) -> list[str]:
    problems: list[str] = []
    items = [
        match.groups()
        for line in _lines_outside_fences(self_test)
        if (match := _CHECKBOX.match(line.strip())) is not None
    ]
    for name in required_boxes:
        matches = [
            (state, text)
            for state, text in items
            if text == name or text.startswith(f"{name} ")
        ]
        if len(matches) != 1:
            problems.append(
                f"body: section `{SELF_TEST_SECTION}` must carry exactly one `{name}` box"
                f"; found {len(matches)}"
            )
            continue
        state, text = matches[0]
        if state == "x" and _BOX_EVIDENCE.search(text) is None:
            problems.append(
                f"body: checked `{name}` box must cite its evidence as "
                "head `<sha>` ... report `<path>`"
            )
    return problems


def check_title(title: str, title_types: Sequence[str]) -> list[str]:
    problems: list[str] = []
    if title != title.strip():
        problems.append("title: leading or trailing whitespace")
    match = re.fullmatch(r"\[([^\]]+)\] (\S.*)", title.strip())
    if match is None:
        problems.append("title: must match `[类型] 简短描述`")
        return problems
    if match.group(1) not in title_types:
        problems.append(
            f"title: unknown type `{match.group(1)}`; allowed: {', '.join(title_types)}"
        )
    return problems


def check_body(
    body: str,
    required_sections: Sequence[str],
    placeholders: Sequence[str],
    required_boxes: Sequence[str],
    *,
    phase: str = "final",
) -> list[str]:
    if phase not in PHASES:
        raise ValueError(f"unknown PR body phase: {phase}")

    problems: list[str] = []
    body_lines = _lines_outside_fences(body)

    # Scan only outside code fences: unchecked checkboxes and HTML comments exempt fenced content by
    # the same rule; if placeholders scanned the raw body, a PR that legitimately quotes a template
    # fragment would be flagged as residue.
    outside_fences = "\n".join(body_lines)
    for token in placeholders:
        if token in outside_fences:
            problems.append(f"body: template placeholder left in place: `{token}`")
    if any("<!--" in line for line in body_lines):
        problems.append("body: HTML comments must not appear outside code fences")

    sections = split_h2_sections(body)
    section_titles = [title for title, _content in sections]
    expected_sections = list(required_sections)
    if section_titles != expected_sections:
        problems.append(
            "body: H2 sections must exactly match the template order: "
            + " → ".join(expected_sections)
            + f"; found: {' → '.join(section_titles) if section_titles else '(none)'}"
        )
        return problems

    for title, content in sections:
        content_lines = _lines_outside_fences(content)
        if not any(line.strip() for line in content_lines):
            problems.append(f"body: section `{title}` is empty")

    for title, content in sections:
        content_lines = _lines_outside_fences(content)
        checked = [line for line in content_lines if line.lstrip().startswith("- [x] ")]
        unchecked = [
            line for line in content_lines if line.lstrip().startswith("- [ ] ")
        ]
        if title == SELF_TEST_SECTION:
            problems.extend(check_required_boxes(content, required_boxes))
            if phase == "final" and not checked:
                problems.append(
                    f"body: section `{SELF_TEST_SECTION}` needs at least one checked item"
                )
            if phase == "draft" and not unchecked:
                problems.append(
                    f"body: section `{SELF_TEST_SECTION}` needs at least one pending item"
                )
        if unchecked and (phase == "final" or title != SELF_TEST_SECTION):
            problems.append(
                f"body: section `{title}` contains unchecked boxes; "
                "remove items that were not performed"
            )
    return problems


def check_goal_contract(body: str, goal_context: str, locked_goal: str) -> list[str]:
    problems: list[str] = []
    try:
        expected_goal, expected_non_goal = parse_context(goal_context)
        body_sections = dict(split_h2_sections(body))
        body_goal, body_non_goal = parse_context(
            "## 目标\n"
            + body_sections[GOAL_SECTION]
            + "## 非目标\n"
            + body_sections[NON_GOAL_SECTION]
        )
    except (KeyError, ValueError) as error:
        return [f"body: invalid goal context: {error}"]
    if body_goal != expected_goal:
        problems.append("body: section `目标` must exactly equal the prepared context")
    if body_non_goal != expected_non_goal:
        problems.append(
            "body: section `非目标` must exactly equal the prepared context"
        )
    if locked_goal != f"{expected_goal}\n":
        problems.append(
            "locked goal: file must exactly equal the prepared `目标` plus one newline"
        )
    return problems


def run(
    template_path: Path,
    title: str,
    body_path: Path,
    *,
    goal_context_path: Path,
    locked_goal_path: Path,
    phase: str = "final",
    body_template_path: Path = DEFAULT_BODY_TEMPLATE,
) -> list[str]:
    title_types = parse_title_types(template_path.read_text(encoding="utf-8"))
    required_sections, placeholders, required_boxes = parse_body_template(
        body_template_path.read_text(encoding="utf-8")
    )
    body = body_path.read_text(encoding="utf-8")
    goal_context = goal_context_path.read_text(encoding="utf-8")
    locked_goal = locked_goal_path.read_text(encoding="utf-8")
    return [
        *check_title(title, title_types),
        *check_body(body, required_sections, placeholders, required_boxes, phase=phase),
        *check_goal_contract(body, goal_context, locked_goal),
    ]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate a PR title and body against the PR templates"
    )
    parser.add_argument("--template", required=True)
    parser.add_argument("--body-template", default=str(DEFAULT_BODY_TEMPLATE))
    parser.add_argument("--title", required=True)
    parser.add_argument("--body-file", required=True)
    parser.add_argument("--goal-context-file", required=True)
    parser.add_argument("--locked-goal-file", required=True)
    parser.add_argument("--phase", choices=PHASES, default="final")
    args = parser.parse_args(argv)
    try:
        problems = run(
            Path(args.template).resolve(),
            args.title,
            Path(args.body_file).resolve(),
            goal_context_path=Path(args.goal_context_file).resolve(),
            locked_goal_path=Path(args.locked_goal_file).resolve(),
            phase=args.phase,
            body_template_path=Path(args.body_template).resolve(),
        )
    except (ValueError, OSError) as error:
        print(f"pr format check failed: {error}", file=sys.stderr)
        return 1
    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1
    print("pr format: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
