"""计算 task-local contract/evidence hash。"""

from __future__ import annotations

import hashlib
import os
import re
from pathlib import Path

from task_graph_parser import fence_marker


def _line_fence_states(lines: list[str]) -> list[bool]:
    states: list[bool] = []
    fence: tuple[str, int] | None = None
    for line in lines:
        marker = fence_marker(line)
        states.append(fence is not None or marker is not None)
        if fence is None and marker is not None:
            fence = marker
        elif (
            fence is not None
            and marker is not None
            and marker[0] == fence[0]
            and marker[1] >= fence[1]
        ):
            fence = None
    return states


def extract_title(text: str) -> str:
    lines = text.splitlines(keepends=True)
    states = _line_fence_states([line.rstrip("\n") for line in lines])
    for line, fenced in zip(lines, states, strict=True):
        if not fenced and line.startswith("# "):
            return line
    raise ValueError("计划缺少标题")


def extract_section(text: str, heading: str) -> str:
    lines = text.splitlines(keepends=True)
    states = _line_fence_states([line.rstrip("\n") for line in lines])
    start: int | None = None
    for index, (line, fenced) in enumerate(zip(lines, states, strict=True)):
        stripped = line.rstrip("\r\n")
        if not fenced and stripped == f"## {heading}":
            start = index
            continue
        if (
            start is not None
            and index > start
            and not fenced
            and re.match(r"^##\s+", line)
        ):
            return "".join(lines[start:index])
    if start is not None:
        return "".join(lines[start:])
    raise ValueError(f"计划缺少 ## {heading}")


def canonical_context(plan_text: str) -> str:
    return (
        "\n".join(
            (
                extract_title(plan_text).rstrip(),
                extract_section(plan_text, "目标").rstrip(),
                extract_section(plan_text, "非目标").rstrip(),
                extract_section(plan_text, "已归档的决策").rstrip(),
            )
        )
        + "\n"
    )


def scope_hashes(
    *,
    context: str,
    owner_file_contract: str,
    task_block: str,
    producer_blocks: tuple[str, ...],
    files: list[tuple[Path, str]],
    evidence_hash: str,
) -> tuple[str, str]:
    context_hash = hashlib.sha256(context.encode()).hexdigest()
    digest = hashlib.sha256()
    digest.update(b"PLAN_CONTEXT_HASH\0")
    digest.update(context_hash.encode())
    digest.update(b"\0OWNER_FILE_CONTRACT\0")
    digest.update(owner_file_contract.encode())
    digest.update(b"\0TASK_BLOCK\0")
    digest.update(task_block.encode())
    for producer in producer_blocks:
        digest.update(b"\0DIRECT_CONSUMED_CONTRACT\0")
        digest.update(producer.encode())
    for path, relative in sorted(files, key=lambda item: item[1]):
        if path.is_symlink():
            data = b"<symlink>\0" + os.fsencode(os.readlink(path))
        elif path.is_file():
            data = b"<file>\0" + path.read_bytes()
        else:
            data = b"<deleted>"
        digest.update(b"\0FILE\0")
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(str(len(data)).encode())
        digest.update(b"\0")
        digest.update(data)
    digest.update(b"\0TASK_EVIDENCE\0")
    digest.update(evidence_hash.encode())
    return context_hash, digest.hexdigest()
