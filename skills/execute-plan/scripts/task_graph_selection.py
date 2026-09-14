"""任务图消费者共享的机械 task identity 选择。"""

from __future__ import annotations

from task_graph_parser import ParsedTaskBlock


def select_task_block(
    blocks: list[ParsedTaskBlock],
    *,
    task_id: int | None = None,
    task_name: str | None = None,
) -> ParsedTaskBlock:
    """按唯一机械 id 或兼容名称选择任务块。"""

    if (task_id is None) == (task_name is None):
        raise ValueError("必须且只能提供 task_id 或 task_name")
    if task_id is not None:
        matches = [block for block in blocks if block.task.id == task_id]
        identity = f"任务 id {task_id}"
    else:
        matches = [block for block in blocks if block.task.name == task_name]
        identity = f"任务名称 {task_name!r}"
    if not matches:
        raise ValueError(f"{identity} 不存在")
    if len(matches) != 1:
        raise ValueError(f"{identity} 重复：匹配到 {len(matches)} 个任务")
    return matches[0]
