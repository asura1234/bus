#!/usr/bin/env python3
"""生成完整 consumer fallout artifact，并输出按任务聚合的高置信摘要。"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[3]
GRAPH_SCRIPTS = REPO_ROOT / "skills" / "execute-plan" / "scripts"
if str(GRAPH_SCRIPTS) not in sys.path:
    sys.path.insert(0, str(GRAPH_SCRIPTS))

from consumer_fallout_analysis import (  # noqa: E402
    SOURCE_SUFFIXES,
    actionable_summary,
    candidate_reasons_by_source,
    candidate_record,
    high_confidence_summary,
    is_test,
    test_subject_stem,
)
from task_graph_validation import (  # noqa: E402
    _declared_path_value,
    _expand_braced_path,
    _is_concrete_declared_path,
)
from verify_task_graph import (  # noqa: E402
    _scan_section,
    check_task_fields,
    parse_tasks,
    verify,
)


_FILE_ENTRY_RE = re.compile(r"^\s*-\s*\[[ xX]\]\s*\*\*([^*]*文件)\*\*\s*[:：]\s*(.+?)\s*$")
_MODULE_SPECIFIER_RE = re.compile(
    r"(?:\bfrom\s+|\bimport\s*(?:\(\s*)?|\brequire\s*\(\s*|"
    r"\bjest\.(?:mock|doMock)\s*\(\s*)['\"]([^'\"]+)['\"]"
)
_RUST_USE_RE = re.compile(
    r"(?ms)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+(.+?);\s*$"
)
_RUST_PATH_RE = re.compile(
    r"(?<![A-Za-z0-9_])(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)"
    r"(?:::[A-Za-z_][A-Za-z0-9_]*)+"
)
_RUST_MOD_RE = re.compile(
    r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)


class InventoryError(Exception):
    """计划或 Git inventory 无法确定性读取。"""


def _git_files(repo: Path) -> list[str]:
    result = subprocess.run(
        ["git", "-C", str(repo), "ls-files", "-z"],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise InventoryError(result.stderr.decode(errors="replace").strip() or "git ls-files 失败")
    return sorted(field.decode(errors="surrogateescape") for field in result.stdout.split(b"\0") if field)


def _declared_files(plan_text: str) -> list[tuple[str, str]]:
    lines, found, errors = _scan_section(plan_text, "需要修改/添加的文件")
    if not found:
        raise InventoryError("计划缺少『需要修改/添加的文件』")
    if errors:
        raise InventoryError("；".join(errors))
    entries: list[tuple[str, str]] = []
    for line in lines:
        if line.fenced:
            continue
        match = _FILE_ENTRY_RE.fullmatch(line.text)
        if not match:
            continue
        action = match.group(1).strip()
        value = _declared_path_value(match.group(2))
        for path in _expand_braced_path(value):
            if _is_concrete_declared_path(path):
                entries.append((action, path))
    return entries


def _read_text(path: Path) -> str | None:
    try:
        return path.read_text()
    except (OSError, UnicodeError):
        return None


def _split_rust_use_items(value: str) -> list[str]:
    items: list[str] = []
    depth = 0
    start = 0
    for index, character in enumerate(value):
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
        elif character == "," and depth == 0:
            items.append(value[start:index])
            start = index + 1
    items.append(value[start:])
    return items


def _expand_rust_use_tree(value: str, prefix: str = "") -> set[str]:
    """Expand Rust brace imports into path-shaped module specifiers."""

    value = re.sub(r"\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*$", "", value.strip())
    if not value:
        return set()
    brace = value.find("{")
    if brace < 0:
        if value == "self":
            return {prefix} if prefix else set()
        return {"::".join(part for part in (prefix, value) if part)}

    depth = 0
    closing = -1
    for index in range(brace, len(value)):
        if value[index] == "{":
            depth += 1
        elif value[index] == "}":
            depth -= 1
            if depth == 0:
                closing = index
                break
    if closing < 0:
        return set()

    head = value[:brace].rstrip(":").strip()
    nested_prefix = "::".join(part for part in (prefix, head) if part)
    expanded: set[str] = set()
    for item in _split_rust_use_items(value[brace + 1 : closing]):
        expanded.update(_expand_rust_use_tree(item, nested_prefix))
    return expanded


def _module_specifiers(path: str, text: str) -> tuple[str, ...]:
    """Return import-like references for the repository's supported languages."""

    specifiers = set(_MODULE_SPECIFIER_RE.findall(text))
    if Path(path).suffix != ".rs":
        return tuple(sorted(specifiers))

    for use_body in _RUST_USE_RE.findall(text):
        specifiers.update(_expand_rust_use_tree(use_body))
    specifiers.update(_RUST_PATH_RE.findall(text))
    specifiers.update(f"./{name}" for name in _RUST_MOD_RE.findall(text))
    return tuple(sorted(specifiers))


def _atomic_write_json(output: Path, payload: dict[str, Any]) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    serialized = json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    descriptor, temporary = tempfile.mkstemp(
        prefix=f".{output.name}.",
        suffix=".tmp",
        dir=output.parent,
    )
    try:
        with os.fdopen(descriptor, "w") as stream:
            stream.write(serialized)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, output)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def build_inventory(repo: Path, plan: Path) -> dict[str, Any]:
    repo = repo.resolve()
    plan = plan if plan.is_absolute() else repo / plan
    plan = plan.resolve()
    try:
        relative_plan = plan.relative_to(repo).as_posix()
    except ValueError as exc:
        raise InventoryError("计划必须位于 repo 内") from exc
    plan_text = plan.read_text()
    task_errors = check_task_fields(plan_text)
    if task_errors:
        raise InventoryError("计划任务结构无效：" + "；".join(task_errors))
    tasks = parse_tasks(plan_text)
    graph_errors = verify(tasks)
    if graph_errors:
        raise InventoryError("计划任务图无效：" + "；".join(graph_errors))

    declared_entries = _declared_files(plan_text)
    declared_paths = {path for _, path in declared_entries}
    tracked = _git_files(repo)
    texts = {path: text for path in tracked if (text := _read_text(repo / path)) is not None}
    specifiers_by_path = {
        path: _module_specifiers(path, text)
        for path, text in texts.items()
    }
    tests_by_stem: dict[str, list[str]] = {}
    for path in texts:
        if (stem := test_subject_stem(path)) is not None:
            tests_by_stem.setdefault(stem, []).append(path)
    frozen_tests = {
        stem: tuple(sorted(paths))
        for stem, paths in tests_by_stem.items()
    }
    source_entries = tuple(
        (action, source)
        for action, source in sorted(set(declared_entries), key=lambda item: item[1])
        if Path(source).suffix in SOURCE_SUFFIXES and not is_test(source)
    )
    candidates_by_source = candidate_reasons_by_source(
        tuple(source for _, source in source_entries),
        specifiers_by_path,
        frozen_tests,
    )
    inventory: list[dict[str, Any]] = []

    for action, source in source_entries:
        inventory.append(
            {
                "action": action,
                "source": source,
                "candidates": [
                    candidate_record(path, reasons, declared_paths, tasks)
                    for path, reasons in sorted(candidates_by_source[source].items())
                ],
            }
        )

    summary = high_confidence_summary(
        inventory,
        specifiers_by_path,
        frozen_tests,
        declared_paths,
        tasks,
    )
    actionable = actionable_summary(summary)
    return {
        "schema_version": 2,
        "kind": "plan-consumer-fallout-inventory",
        "plan": relative_plan,
        "note": (
            "完整 candidates 只证明文本关系；reviewer 默认消费 high_confidence_summary，"
            "并对 unresolved 项裁决 owner 或明确排除"
        ),
        "actionable_summary": actionable,
        "high_confidence_summary": summary,
        "entries": inventory,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--output", type=Path, help="完整 JSON artifact；提供时 stdout 只输出高置信摘要")
    args = parser.parse_args(argv)
    try:
        payload = build_inventory(args.repo, args.plan)
        if args.output is not None:
            _atomic_write_json(args.output, payload)
    except (OSError, UnicodeError, ValueError, InventoryError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    output = payload if args.output is None else dict(
        schema_version=payload["schema_version"],
        kind="plan-consumer-fallout-summary",
        plan=payload["plan"],
        artifact=str(args.output),
        **payload["actionable_summary"],
    )
    print(json.dumps(output, ensure_ascii=False, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
