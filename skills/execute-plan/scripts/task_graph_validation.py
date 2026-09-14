#!/usr/bin/env python3
"""任务图的字段、文件契约、依赖环、owner 隔离与 wave 校验。"""

from __future__ import annotations

import datetime
import re
from pathlib import PurePosixPath

from task_graph_parser import (
    _BACKTICK_VALUE_RE,
    _DECLARED_PATH_OWNER_REQUIRED_SINCE,
    _FILE_CONTRACT_ENTRY_RE,
    _GATE_LEVEL_REQUIRED_SINCE,
    _GLOB_CHARS_RE,
    _MANUAL_MARKER,
    _OLD_TOKEN_RE,
    _TEST_FILE_HEADING_RE,
    CREATED_DATE_LABEL_RE,
    CREATED_DATE_RE,
    Task,
    _parse_all,
    _scan_section,
)


def check_task_fields(plan_text: str) -> list[str]:
    """检查任务块字段、身份以及旧格式或混合格式。"""

    _, blocks, parse_errors, section_lines, found_section = _parse_all(plan_text)
    old_tokens = [line for line in section_lines if not line.fenced and _OLD_TOKEN_RE.search(line.text)]
    manual_markers = [
        line
        for block in blocks
        for line in [block.header, *block.lines]
        if not line.fenced and _MANUAL_MARKER in line.text
    ]
    errors: list[str] = []

    if not found_section or not blocks:
        errors.extend(parse_errors)
        if old_tokens:
            errors.append("旧格式不支持重跑，请先把线性阶段/步骤计划迁移为任务图")
        else:
            errors.append("实施步骤中无任务块")
        return errors

    if old_tokens:
        errors.append("实施步骤同时存在任务块与旧线性 token，属于混合态")
    if manual_markers:
        lines = "、".join(str(line.number) for line in manual_markers)
        errors.append(f"实施任务含 (需要手动操作) 标记（行 {lines}）")
    errors.extend(parse_errors)

    ids = [block.task_id for block in blocks]
    if ids != list(range(1, len(ids) + 1)):
        errors.append("任务 id 必须从 1 开始、按源码顺序连续且不得重复")

    names = [block.name for block in blocks if block.name]
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        errors.append(f"任务名称重复：{'、'.join(duplicates)}")

    return errors


def _canonical_owned_path(path: str) -> tuple[str, ...]:
    pure = PurePosixPath(path)
    parts = list(pure.parts)
    if parts and parts[-1] in {"AGENTS.md", "CLAUDE.md"}:
        parts[-1] = "__AI_INSTRUCTIONS__"
    return tuple(parts)


def _paths_overlap(left: str, right: str) -> bool:
    left_parts = _canonical_owned_path(left)
    right_parts = _canonical_owned_path(right)
    shorter = min(len(left_parts), len(right_parts))
    return left_parts[:shorter] == right_parts[:shorter]


def _expand_braced_path(path: str) -> list[str]:
    """展开文件契约使用的简单 `{a,b}` 路径写法；支持多个顺序 brace group。"""

    start = path.find("{")
    if start < 0:
        return [path]
    end = path.find("}", start + 1)
    if end < 0:
        return [path]
    options = [item.strip() for item in re.split(r"[,，、]", path[start + 1 : end])]
    if not options or any(not option for option in options):
        return [path]
    expanded: list[str] = []
    for option in options:
        expanded.extend(_expand_braced_path(path[:start] + option + path[end + 1 :]))
    return expanded


def _declared_path_value(raw: str) -> str:
    match = _BACKTICK_VALUE_RE.search(raw)
    if match:
        return match.group(1).strip()
    return raw.strip().split(maxsplit=1)[0] if raw.strip() else ""


def _is_concrete_declared_path(path: str) -> bool:
    if not path or path.startswith(("http://", "https://")):
        return False
    if path.startswith("[") and path.endswith("]"):
        return False
    if any(token in path for token in ("<", ">", "…")):
        return False
    return "/" in path or bool(PurePosixPath(path).suffix)


def _section_declared_paths(plan_text: str, heading: str, pattern: re.Pattern[str]) -> tuple[set[str], list[str]]:
    lines, found_section, scan_errors = _scan_section(plan_text, heading)
    if not found_section:
        return set(), scan_errors
    paths: set[str] = set()
    for line in lines:
        if line.fenced:
            continue
        match = pattern.fullmatch(line.text)
        if not match:
            continue
        value = _declared_path_value(match.group(1))
        for expanded in _expand_braced_path(value):
            if _is_concrete_declared_path(expanded):
                paths.add(expanded)
    return paths, scan_errors


def _owner_covers_path(owner: str, path: str) -> bool:
    owner_parts = _canonical_owned_path(owner)
    path_parts = _canonical_owned_path(path)
    return len(owner_parts) <= len(path_parts) and path_parts[: len(owner_parts)] == owner_parts


def _contract_matches_path(contract_path: str, path: str) -> bool:
    return _canonical_owned_path(contract_path) == _canonical_owned_path(path)


def file_contract_for_owners(plan_text: str, owners: tuple[str, ...]) -> str:
    """返回 owner 相交的逐文件契约块；供 task-local evidence 精确失效。"""

    lines, found_section, scan_errors = _scan_section(plan_text, "需要修改/添加的文件")
    if scan_errors:
        raise ValueError("\n".join(scan_errors))
    if not found_section:
        return ""
    chunks: list[tuple[list[str], list[str]]] = []
    current_lines: list[str] = []
    current_paths: list[str] = []
    for line in lines:
        match = None if line.fenced else _FILE_CONTRACT_ENTRY_RE.fullmatch(line.text)
        if match:
            if current_lines:
                chunks.append((current_paths, current_lines))
            value = _declared_path_value(match.group(1))
            current_paths = [path for path in _expand_braced_path(value) if _is_concrete_declared_path(path)]
            current_lines = [line.raw]
        elif current_lines:
            current_lines.append(line.raw)
    if current_lines:
        chunks.append((current_paths, current_lines))
    selected = [
        "".join(chunk_lines)
        for paths, chunk_lines in chunks
        if any(_paths_overlap(owner, path) for owner in owners for path in paths)
    ]
    return "".join(selected)


def _requires_plan_path_closure(plan_text: str) -> bool:
    return _date_requires(plan_text, _DECLARED_PATH_OWNER_REQUIRED_SINCE)


def check_plan_path_closure(plan_text: str, tasks: list[Task]) -> list[str]:
    """校验新计划的显式文件/测试路径已闭合到文件契约和唯一 task owner。

    只处理可确定性证明的 literal/brace-expanded 路径；glob 测试模式交给语义 review。
    Owner 可为稳定目录边界，因此 owner 检查使用 containment；测试文件必须逐文件进入
    文件契约，目录级文件契约不能替代显式测试文件条目。
    """

    if not _requires_plan_path_closure(plan_text):
        return []

    file_contract_paths, file_contract_errors = _section_declared_paths(
        plan_text, "需要修改/添加的文件", _FILE_CONTRACT_ENTRY_RE
    )
    test_paths, test_plan_errors = _section_declared_paths(plan_text, "测试计划", _TEST_FILE_HEADING_RE)
    literal_test_paths = {path for path in test_paths if not _GLOB_CHARS_RE.search(path)}
    owners = tuple(path for task in tasks for path in task.owned_files)
    errors = list(dict.fromkeys((*file_contract_errors, *test_plan_errors)))

    for path in sorted(file_contract_paths):
        if not any(_owner_covers_path(owner, path) for owner in owners):
            errors.append(f"文件契约路径未被任何任务 owner 覆盖：{path}")

    for path in sorted(literal_test_paths):
        if not any(_contract_matches_path(contract_path, path) for contract_path in file_contract_paths):
            errors.append(f"测试文件未进入『需要修改/添加的文件』契约：{path}")
        if not any(_owner_covers_path(owner, path) for owner in owners):
            errors.append(f"测试文件未被任何任务 owner 覆盖：{path}")
    return errors


def _date_requires(plan_text: str, cutoff_value: str) -> bool:
    if not CREATED_DATE_LABEL_RE.search(plan_text):
        return False
    created = CREATED_DATE_RE.search(plan_text)
    if not created:
        return True
    try:
        created_date = datetime.date.fromisoformat(created.group(1))
    except ValueError:
        return True
    return created_date >= datetime.date.fromisoformat(cutoff_value)


def check_gate_execution_levels(plan_text: str, tasks: list[Task]) -> list[str]:
    """校验新计划的 task gate 层级及可确定识别的昂贵重复。"""

    if not _date_requires(plan_text, _GATE_LEVEL_REQUIRED_SINCE):
        return []
    errors: list[str] = []
    seen_commands: dict[str, int] = {}
    broad_commands = {
        "just build",
        "just lint",
        "just test",
        "just ci",
        "just check",
        "cargo test --all-targets --locked",
        "cargo clippy --all-targets --locked -- -D warnings",
    }
    for task in tasks:
        if task.gate_level != "TASK_LOCAL":
            errors.append(f"任务{task.id}（{task.name}）验收闸门必须以 [TASK_LOCAL] 开头")
        if not task.gate_commands:
            errors.append(f"任务{task.id}（{task.name}）验收闸门缺少反引号精确命令")
        for command in task.gate_commands:
            if command in broad_commands:
                errors.append(f"任务{task.id}（{task.name}）TASK_LOCAL 不得运行 final-tree 广域命令：{command}")
            previous = seen_commands.get(command)
            if previous is not None:
                errors.append(f"TASK_LOCAL 命令跨任务重复：任务{previous} 与任务{task.id} 均运行 {command}")
            else:
                seen_commands[command] = task.id
    return errors


def _path_errors(task: Task, path: str) -> list[str]:
    errors: list[str] = []
    if _GLOB_CHARS_RE.search(path):
        errors.append(f"任务{task.id}（{task.name}）拥有文件含 glob 字符：{path}")
    pure = PurePosixPath(path)
    if pure.is_absolute() or ".." in pure.parts:
        errors.append(f"任务{task.id}（{task.name}）拥有文件必须是 repo-relative 路径：{path}")
    return errors


def _cycle_errors(tasks: list[Task], task_by_id: dict[int, Task]) -> list[str]:
    index = 0
    indices: dict[int, int] = {}
    low_links: dict[int, int] = {}
    stack: list[int] = []
    on_stack: set[int] = set()
    components: list[list[int]] = []

    def visit(task_id: int) -> None:
        nonlocal index
        indices[task_id] = index
        low_links[task_id] = index
        index += 1
        stack.append(task_id)
        on_stack.add(task_id)

        for dependency in sorted(task_by_id[task_id].blocked_by):
            if dependency not in task_by_id:
                continue
            if dependency not in indices:
                visit(dependency)
                low_links[task_id] = min(low_links[task_id], low_links[dependency])
            elif dependency in on_stack:
                low_links[task_id] = min(low_links[task_id], indices[dependency])

        if low_links[task_id] != indices[task_id]:
            return
        component: list[int] = []
        while True:
            member = stack.pop()
            on_stack.remove(member)
            component.append(member)
            if member == task_id:
                break
        components.append(sorted(component))

    for task in sorted(tasks, key=lambda item: item.id):
        if task.id not in indices:
            visit(task.id)

    errors: list[str] = []
    for component in sorted(components, key=lambda item: item[0]):
        self_loop = len(component) == 1 and component[0] in task_by_id[component[0]].blocked_by
        if len(component) == 1 and not self_loop:
            continue
        members = "、".join(f"任务{task_id}（{task_by_id[task_id].name}）" for task_id in component)
        errors.append(f"任务图存在环：{members}；请将环内任务合并为一个任务")
    return errors


def verify(tasks: list[Task]) -> list[str]:
    """聚合校验依赖图、单一写 owner 与 consumes 边一致性。"""

    if not tasks:
        return []

    errors: list[str] = []
    task_by_id: dict[int, Task] = {}
    duplicate_ids: set[int] = set()
    for task in tasks:
        if task.id in task_by_id:
            duplicate_ids.add(task.id)
        else:
            task_by_id[task.id] = task
    if duplicate_ids:
        errors.append("任务 id 重复：" + "、".join(f"任务{task_id}" for task_id in sorted(duplicate_ids)))

    for task in tasks:
        if not task.owned_files:
            errors.append(f"任务{task.id}（{task.name}）拥有文件不能为空")
        for path in task.owned_files:
            errors.extend(_path_errors(task, path))
        for dependency in task.blocked_by:
            if dependency not in task_by_id:
                errors.append(f"任务{task.id}（{task.name}）blocked-by 引用不存在的任务{dependency}")
        for producer in task.consumes:
            if producer not in task.blocked_by:
                errors.append(f"任务{task.id}（{task.name}）consumes 任务{producer}，但 blocked-by 缺少对应依赖边")

    for index, left in enumerate(tasks):
        for right in tasks[index + 1 :]:
            for left_path in left.owned_files:
                for right_path in right.owned_files:
                    if _paths_overlap(left_path, right_path):
                        errors.append(
                            f"拥有文件冲突：任务{left.id}（{left.name}）的 {left_path} 与 "
                            f"任务{right.id}（{right.name}）的 {right_path} 相交"
                        )

    if not duplicate_ids:
        errors.extend(_cycle_errors(tasks, task_by_id))
    return errors


def emit_waves(tasks: list[Task]) -> list[list[str]]:
    """按 blocked-by 生成确定性 wave；每个 wave 内按任务 id 升序。"""

    if not tasks:
        return []
    errors = verify(tasks)
    if errors:
        raise ValueError("任务图无效：\n" + "\n".join(f"- {error}" for error in errors))

    remaining = {task.id: set(task.blocked_by) for task in tasks}
    waves: list[list[str]] = []
    emitted: set[int] = set()

    while len(emitted) < len(tasks):
        ready = sorted(
            task_id for task_id, dependencies in remaining.items() if task_id not in emitted and dependencies <= emitted
        )
        if not ready:
            raise ValueError("任务图无效：无法生成 wave")
        waves.append([f"任务{task_id}" for task_id in ready])
        emitted.update(ready)

    return waves
