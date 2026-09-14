#!/usr/bin/env python3
"""为 /execute-plan 生成 legacy 全计划或 task-scoped 计划快照。

无任务 selector 时保持既有剥离行为；指定 ``--task-id`` 或兼容 ``--task`` 时只保留 canonical task contract：
标题、逐字目标/非目标、完整归档决策、完整文件契约与一个精确匹配的任务块。

用法：strip_plan.py <input> [output] [--task-id <id> | --task <unique-name>]
"""

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from task_graph_parser import fence_marker
from task_graph_selection import select_task_block
from verify_task_graph import parse_task_blocks, parse_tasks


SECTIONS_TO_SKIP = frozenset(
    [
        "## 规则优先级",
        "## 计划生成规则",
        "## 计划执行规则",
        "## 需要决策的事项",
        "## 已归档的决策",
    ]
)

BLOCKQUOTE_NOISE_RE = re.compile(r"^>\s*\*\*(大小说明|机械性变更降级规则|状态说明|语言无关说明|重要|注意)\*\*")

_CANONICAL_SECTIONS = (
    "目标",
    "非目标",
    "已归档的决策",
    "需要修改/添加的文件",
)


@dataclass(frozen=True)
class _Heading:
    level: int
    title: str
    line_index: int


def _headings(lines: list[str]) -> list[_Heading]:
    headings: list[_Heading] = []
    fence: tuple[str, int] | None = None
    for index, line in enumerate(lines):
        marker = fence_marker(line)
        if fence is None and marker is not None:
            fence = marker
            continue
        if fence is not None:
            if marker is not None and marker[0] == fence[0] and marker[1] >= fence[1]:
                fence = None
            continue

        match = re.match(r"^(#{1,2})\s+(.+?)\s*(?:\r?\n)?$", line)
        if match:
            headings.append(_Heading(len(match.group(1)), match.group(2).strip(), index))
    return headings


def _unique_title(lines: list[str], headings: list[_Heading]) -> str:
    titles = [heading for heading in headings if heading.level == 1]
    if not titles:
        raise ValueError("计划缺少一级标题")
    if len(titles) != 1:
        raise ValueError(f"计划一级标题重复：共 {len(titles)} 个")
    return lines[titles[0].line_index]


def _unique_section(lines: list[str], headings: list[_Heading], title: str) -> tuple[str, int, int]:
    matches = [heading for heading in headings if heading.level == 2 and heading.title == title]
    if not matches:
        raise ValueError(f"计划缺少 ## {title} section")
    if len(matches) != 1:
        raise ValueError(f"section {title} 重复：共 {len(matches)} 个")

    start = matches[0].line_index
    end = next(
        (heading.line_index for heading in headings if heading.level == 2 and heading.line_index > start),
        len(lines),
    )
    return "".join(lines[start:end]), start, end


def _append_piece(result: str, piece: str) -> str:
    if result:
        if result.endswith("\n\n"):
            pass
        elif result.endswith("\n"):
            result += "\n"
        else:
            result += "\n\n"
    return result + piece


def _strip_plan_for_selected_task(
    plan_content: str,
    *,
    task_id: int | None = None,
    task_name: str | None = None,
) -> str:
    """生成只含一个机械选定任务及其 canonical 上下文的快照。"""

    lines = plan_content.splitlines(keepends=True)
    headings = _headings(lines)
    title = _unique_title(lines, headings)
    sections = [_unique_section(lines, headings, section_title)[0] for section_title in _CANONICAL_SECTIONS]
    _, implementation_start, _ = _unique_section(lines, headings, "实施步骤")

    try:
        task_blocks = parse_task_blocks(plan_content)
    except ValueError as error:
        raise ValueError(f"任务图解析失败：{error}") from error

    try:
        selected = select_task_block(task_blocks, task_id=task_id, task_name=task_name)
    except ValueError as exc:
        if task_name is not None and "不存在" in str(exc):
            raise ValueError(f"{exc}；只接受精确匹配，不接受模糊名称") from exc
        raise

    implementation_heading = lines[implementation_start]
    task_block = selected.text

    result = title
    for section in sections:
        result = _append_piece(result, section)
    result = _append_piece(result, implementation_heading)
    result = _append_piece(result, task_block)
    if not result.endswith("\n"):
        result += "\n"

    try:
        output_tasks = parse_tasks(result)
    except ValueError as error:
        raise ValueError(f"task snapshot 解析失败：{error}") from error
    if len(output_tasks) != 1 or output_tasks[0].id != selected.task.id:
        raise ValueError("task snapshot 必须恰好包含一个指定任务")
    return result


def strip_plan_for_task(plan_content: str, task_name: str) -> str:
    """兼容按精确任务名称生成 task snapshot。"""

    return _strip_plan_for_selected_task(plan_content, task_name=task_name)


def strip_plan_for_task_id(plan_content: str, task_id: int) -> str:
    """按计划中的连续机械 id 生成 task snapshot。"""

    return _strip_plan_for_selected_task(plan_content, task_id=task_id)


def strip_plan_for_execution(plan_content: str) -> str:
    lines = plan_content.split("\n")
    result: list[str] = []
    skip_section = False
    skip_blockquote = False
    in_header = True

    i = 0
    while i < len(lines):
        line = lines[i]

        if in_header:
            if line.startswith("# "):
                in_header = False
                result.append(line)
                i += 1
                continue
            i += 1
            continue

        if "<!--" in line:
            if "-->" in line:
                i += 1
                continue
            while i < len(lines) and "-->" not in lines[i]:
                i += 1
            i += 1
            continue

        if any(line.startswith(s) for s in SECTIONS_TO_SKIP):
            skip_section = True
            i += 1
            continue

        if skip_section and line.startswith("## "):
            skip_section = False

        if skip_section:
            i += 1
            continue

        if BLOCKQUOTE_NOISE_RE.match(line):
            skip_blockquote = True
            i += 1
            continue

        if skip_blockquote:
            if line.startswith(">"):
                i += 1
                continue
            skip_blockquote = False

        result.append(line)
        i += 1

    cleaned = "\n".join(result)
    cleaned = re.sub(r"\n{3,}", "\n\n", cleaned)
    return cleaned.strip() + "\n"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog=Path(argv[0]).name if argv else "strip_plan.py",
        description="生成 legacy 全计划或 task-scoped 计划快照",
    )
    parser.add_argument("input_plan", help="输入计划 Markdown")
    parser.add_argument("output_stripped", nargs="?", help="输出文件；省略时写 stdout")
    selector = parser.add_mutually_exclusive_group()
    selector.add_argument("--task-id", type=int, help="按机械任务 id 生成 task-scoped snapshot")
    selector.add_argument("--task", help="兼容按唯一任务名称生成 task-scoped snapshot")
    try:
        args = parser.parse_args(argv[1:])
    except SystemExit as error:
        return int(error.code)

    input_path = Path(args.input_plan)
    if not input_path.is_file():
        print(f"error: {input_path} not found", file=sys.stderr)
        return 1

    try:
        plan_content = input_path.read_text()
        if args.task_id is not None:
            stripped = strip_plan_for_task_id(plan_content, args.task_id)
        elif args.task is not None:
            stripped = strip_plan_for_task(plan_content, args.task)
        else:
            stripped = strip_plan_for_execution(plan_content)
    except (OSError, UnicodeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    try:
        if args.output_stripped is not None:
            output_path = Path(args.output_stripped)
            output_path.parent.mkdir(parents=True, exist_ok=True)
            output_path.write_text(stripped)
        else:
            sys.stdout.write(stripped)
    except (OSError, UnicodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
