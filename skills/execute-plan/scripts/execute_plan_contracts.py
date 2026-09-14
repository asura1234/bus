#!/usr/bin/env python3
"""execute-plan 共用的完整任务图校验与 task contract identity。"""

from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from execute_plan_common import ExecutePlanRuntimeError  # noqa: E402
from task_graph_validation import (  # noqa: E402
    check_gate_execution_levels,
    check_plan_path_closure,
    file_contract_for_owners,
    verify,
)
from task_scope_evidence import canonical_context  # noqa: E402
from verify_task_graph import (  # noqa: E402
    check_task_fields,
    parse_task_blocks,
    parse_tasks,
)


def validated_plan_tasks(plan_text: str) -> list[Any]:
    """执行与 standalone verifier 相同的完整任务图校验。"""

    errors = check_task_fields(plan_text)
    tasks: list[Any] = []
    if not errors:
        try:
            tasks = parse_tasks(plan_text)
        except ValueError as error:
            errors.append(str(error))
        else:
            errors.extend(verify(tasks))
            errors.extend(check_plan_path_closure(plan_text, tasks))
            errors.extend(check_gate_execution_levels(plan_text, tasks))
    if errors:
        raise ExecutePlanRuntimeError("计划任务图无效：" + "；".join(errors))
    return tasks


def _normalized_task_block(text: str) -> str:
    return re.sub(
        r"(?m)^(\s*-\s*)\[[ xX]\](\s*\*\*完成\*\*\s*)$",
        r"\1[ ]\2",
        text,
        count=1,
    )


def _normalized_file_contract(text: str) -> str:
    return re.sub(
        r"(?m)^(\s*-\s*)\[[ xX]\](\s*)",
        r"\1[ ]\2",
        text,
    )


def task_contract_identities(
    plan_text: str,
) -> tuple[list[Any], dict[int, str]]:
    """绑定全局目标、task block 与其 owner 对应文件契约。"""

    tasks = validated_plan_tasks(plan_text)
    blocks = {block.task.id: block for block in parse_task_blocks(plan_text)}
    context = canonical_context(plan_text)
    identities: dict[int, str] = {}
    for task in tasks:
        digest = hashlib.sha256()
        digest.update(b"PLAN_CONTEXT\0")
        digest.update(context.encode())
        digest.update(b"\0TASK_BLOCK\0")
        digest.update(_normalized_task_block(blocks[task.id].text).encode())
        digest.update(b"\0OWNER_FILE_CONTRACT\0")
        digest.update(
            _normalized_file_contract(
                file_contract_for_owners(plan_text, task.owned_files)
            ).encode()
        )
        identities[task.id] = digest.hexdigest()
    return tasks, identities
