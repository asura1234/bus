#!/usr/bin/env python3
"""pr skill 的机械层：校验 PR 标题与正文是否符合 references/pr-template.md。

模板是唯一格式事实源：标题类型枚举取自模板的 `**类型**：` 行，正文必备 section 取自模板
`# PR 描述` 之后的全部 H2 标题。本脚本只证明结构与勾选状态，不判断内容质量。

校验规则（全部 fail-closed）：
- 标题形如 `[类型] 简短描述`，类型属于模板枚举，描述非空且无首尾空白。
- 正文 H2 section 与模板完全一致（同集合、同顺序），且每节非空。
- `draft` 阶段允许「文档同步」与「自测 / Agent 测」保留 `- [ ]` pending 项；其他 section
  仍禁止未勾选项，且「文档同步」只能包含 pending 项。
- `final` 阶段的「自测 / Agent 测」至少一个 checkbox 且全部为 `- [x]`；全文禁止
  未勾选项；「文档同步」的每个条目都必须是 renderer 输出形态的 `- [x] `。
- 全文（代码块外）不允许 HTML 注释或模板占位符残留。
- `摘要` 必须含合法的 `- **大小**：` 档位。
- `目标` / `非目标` 必须逐字等于机械生成的 context，review-pr lock 必须等于目标。
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence

from pr_goal_context import parse_context, split_h2_sections


SELF_TEST_SECTION = "自测 / Agent 测"
DOCS_SYNC_SECTION = "文档同步"
SUMMARY_SECTION = "摘要"
GOAL_SECTION = "目标"
NON_GOAL_SECTION = "非目标"
PHASES = ("draft", "final")
SIZE_PATTERN = re.compile(r"^- \*\*大小\*\*：`(XS|S|M|L|XL)`", re.MULTILINE)
# 占位符从模板正文里派生，不写死清单：写死的那份只覆盖当时想到的几个，模板后来新增的
# 占位符会静默漏过，而门禁的全部意义就是挡住模板残留。
_PLACEHOLDER_PATTERN = re.compile(r"\[([^\[\]\n]+)\]")
_CHECKBOX_PREFIX = re.compile(r"^\s*[-*]\s*\[[ xX]\]\s*")


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
    """从模板正文提取占位符（形如 `[…]` 的待填槽位）。

    只取 `# PR 描述` 之后的部分：标题小节里的 `[feat]`、`[类型]` 是**标题**格式示例，
    正文完全可能正当地提到它们（比如讨论该用哪个类型），拿它们卡正文会误伤。

    逐行排除三类不是槽位的方括号：代码块内的行、`>` 说明块与 HTML 注释（都是给 AI 的
    指令，不进最终 PR）、以及 `- [x]` checkbox 前缀和 `[文字](链接)` 形式的 Markdown 链接。
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


def parse_template(template_text: str) -> tuple[list[str], list[str], list[str]]:
    """从模板提取 (标题类型枚举, 正文必备 H2 section 列表, 正文占位符列表)。"""
    type_line = next(
        (
            line
            for line in template_text.splitlines()
            if line.startswith("**类型**：")
        ),
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

    lines = template_text.splitlines()
    try:
        description_start = lines.index("# PR 描述")
    except ValueError as error:
        raise ValueError("template is missing the `# PR 描述` heading") from error
    sections = [
        line.removeprefix("## ").strip()
        for line in _lines_outside_fences("\n".join(lines[description_start + 1 :]))
        if line.startswith("## ")
    ]
    if not sections:
        raise ValueError("template defines no body sections")
    description_text = "\n".join(lines[description_start + 1 :])
    return title_types, sections, _derive_body_placeholders(description_text)


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
    *,
    phase: str = "final",
) -> list[str]:
    if phase not in PHASES:
        raise ValueError(f"unknown PR body phase: {phase}")

    problems: list[str] = []
    body_lines = _lines_outside_fences(body)

    # 只扫代码块外：未勾选 checkbox 与 HTML 注释都按同一口径豁免围栏内容，占位符若改扫
    # 原始 body，一份正当引用了模板片段的 PR 会被判成残留。
    outside_fences = "\n".join(body_lines)
    for token in placeholders:
        if token in outside_fences:
            problems.append(f"body: template placeholder left in place: `{token}`")
    if any("<!--" in line for line in body_lines):
        problems.append("body: HTML comments must not appear outside code fences")

    sections = split_h2_sections(body)
    section_titles = [title for title, _content in sections]
    if section_titles != list(required_sections):
        problems.append(
            "body: H2 sections must exactly match the template order: "
            + " → ".join(required_sections)
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
        unchecked = [line for line in content_lines if line.lstrip().startswith("- [ ] ")]
        if title == SELF_TEST_SECTION:
            if phase == "final" and not checked:
                problems.append(
                    f"body: section `{SELF_TEST_SECTION}` needs at least one checked item"
                )
            if phase == "draft" and not unchecked:
                problems.append(
                    f"body: section `{SELF_TEST_SECTION}` needs at least one pending item"
                )
        if unchecked and (phase == "final" or title not in {SELF_TEST_SECTION, DOCS_SYNC_SECTION}):
            problems.append(
                f"body: section `{title}` contains unchecked boxes; "
                "remove items that were not performed"
            )
        if title == DOCS_SYNC_SECTION:
            for line in content_lines:
                expected_prefix = "- [ ] " if phase == "draft" else "- [x] "
                if line.strip() and not line.startswith(expected_prefix):
                    if phase == "final":
                        problems.append(
                            f"body: section `{DOCS_SYNC_SECTION}` must contain only renderer "
                            f"`- [x] ` lines; found: {line.strip()!r}"
                        )
                    else:
                        problems.append(
                            f"body: section `{DOCS_SYNC_SECTION}` must contain only "
                            f"`- [ ] ` lines in draft phase; found: {line.strip()!r}"
                        )
                    break
        if title == SUMMARY_SECTION and SIZE_PATTERN.search("\n".join(content_lines)) is None:
            problems.append(
                "body: section `摘要` must contain `- **大小**：` with one of "
                "`XS|S|M|L|XL`"
            )
    return problems


def check_goal_contract(body: str, goal_context: str, locked_goal: str) -> list[str]:
    problems: list[str] = []
    try:
        expected_goal, expected_non_goal = parse_context(goal_context)
        body_sections = dict(split_h2_sections(body))
        body_goal, body_non_goal = parse_context(
            "## 目标\n" + body_sections[GOAL_SECTION] + "## 非目标\n" + body_sections[NON_GOAL_SECTION]
        )
    except (KeyError, ValueError) as error:
        return [f"body: invalid goal context: {error}"]
    if body_goal != expected_goal:
        problems.append("body: section `目标` must exactly equal the prepared context")
    if body_non_goal != expected_non_goal:
        problems.append("body: section `非目标` must exactly equal the prepared context")
    if locked_goal != f"{expected_goal}\n":
        problems.append("locked goal: file must exactly equal the prepared `目标` plus one newline")
    return problems


def run(
    template_path: Path,
    title: str,
    body_path: Path,
    *,
    goal_context_path: Path,
    locked_goal_path: Path,
    phase: str = "final",
) -> list[str]:
    title_types, required_sections, placeholders = parse_template(
        template_path.read_text(encoding="utf-8")
    )
    body = body_path.read_text(encoding="utf-8")
    goal_context = goal_context_path.read_text(encoding="utf-8")
    locked_goal = locked_goal_path.read_text(encoding="utf-8")
    return [
        *check_title(title, title_types),
        *check_body(body, required_sections, placeholders, phase=phase),
        *check_goal_contract(body, goal_context, locked_goal),
    ]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate a PR title and body against the pr-template SOT"
    )
    parser.add_argument("--template", required=True)
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
