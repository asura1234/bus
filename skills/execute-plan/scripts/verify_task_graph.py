#!/usr/bin/env python3
"""解析计划任务图，校验结构与文件隔离，并输出确定性 wave。"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import task_graph_parser as _parser_module
import task_graph_validation as _validation_module
from final_gate_contract import check_final_gate_contract
from task_graph_parser import (
    Task,
    parse_tasks,
)
from task_graph_validation import (
    check_gate_execution_levels,
    check_plan_path_closure,
    check_task_fields,
    emit_waves,
    verify,
)


for _module in (_parser_module, _validation_module):
    for _name in dir(_module):
        if not _name.startswith("__"):
            globals().setdefault(_name, getattr(_module, _name))
del _module, _name


def _format_waves(tasks: list[Task], waves: list[list[str]]) -> str:
    task_by_id = {task.id: task for task in tasks}
    lines: list[str] = []
    for index, wave in enumerate(waves, start=1):
        entries: list[str] = []
        for token in wave:
            task_id = int(token.removeprefix("任务"))
            task = task_by_id[task_id]
            entry = f"任务{task.id} | {task.name}"
            if task.done:
                entry += " | [done]"
            entries.append(entry)
        lines.append(f"Wave {index}: " + "; ".join(entries))
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="校验计划任务图并输出 wave")
    parser.add_argument("plan", help="计划 Markdown 文件")
    args = parser.parse_args(argv)
    plan_path = Path(args.plan)
    if not plan_path.is_file():
        parser.error(f"计划文件不存在：{plan_path}")

    try:
        plan_text = plan_path.read_text()
    except (OSError, UnicodeError) as error:
        parser.error(f"无法读取计划文件：{error}")

    errors = check_task_fields(plan_text)
    tasks: list[Task] = []
    if not errors:
        try:
            tasks = parse_tasks(plan_text)
        except ValueError as error:
            errors.append(str(error))
        else:
            errors.extend(verify(tasks))
            errors.extend(check_plan_path_closure(plan_text, tasks))
            errors.extend(check_gate_execution_levels(plan_text, tasks))
            errors.extend(check_final_gate_contract(plan_text))

    if errors:
        for error in errors:
            print(f"- {error}")
        return 1

    print(_format_waves(tasks, emit_waves(tasks)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
