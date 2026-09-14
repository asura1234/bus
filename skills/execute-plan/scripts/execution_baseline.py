#!/usr/bin/env python3
"""create-only 固定 execute-plan 起点 HEAD 与预存 dirty baseline。"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from task_graph_validation import _owner_covers_path  # noqa: E402
from workspace_manifest import (  # noqa: E402
    WorkspaceManifestError,
    atomic_create_json,
    canonical_repo_root,
    capture_entries,
    entry_to_dict,
    head,
    stable_json,
)


SCHEMA_VERSION = 1
KIND = "execute-plan-baseline"


class BaselineError(ValueError):
    """baseline identity 或 create-only 复用不合法。"""


def validate_existing(
    repo_raw: Path,
    plan_raw: Path,
    output: Path,
    *,
    allow_head_change: bool = False,
) -> dict[str, object]:
    """复核已发布 baseline；landing 对账可允许 HEAD 合法前进。"""

    repo = canonical_repo_root(repo_raw)
    plan = plan_raw if plan_raw.is_absolute() else repo / plan_raw
    plan = plan.resolve()
    try:
        relative = plan.relative_to(repo).as_posix()
        existing = json.loads(output.read_text())
    except (ValueError, OSError, json.JSONDecodeError) as error:
        raise BaselineError("已有 baseline 无法读取") from error
    if (
        not isinstance(existing, dict)
        or existing.get("schema_version") != SCHEMA_VERSION
        or existing.get("kind") != KIND
        or existing.get("repo_root") != str(repo)
        or existing.get("plan") != relative
        or (
            not allow_head_change
            and existing.get("execution_start_head") != head(repo)
        )
    ):
        raise BaselineError("已有 baseline identity 与当前 execution 不一致")
    current = {entry.path: entry_to_dict(entry) for entry in capture_entries(repo)}
    for entry in existing.get("entries", []):
        if not isinstance(entry, dict) or current.get(entry.get("path")) != entry:
            raise BaselineError("预存 dirty baseline 已变化；resume 不得覆盖")
    return existing


def validate_owner_disjoint(
    baseline: dict[str, object],
    tasks: list[object],
) -> None:
    """预存 dirty path（含 rename 原路径）不得落入任何 task owner。"""

    overlaps: list[str] = []
    entries = baseline.get("entries")
    if not isinstance(entries, list):
        raise BaselineError("baseline entries schema 不合法")
    owners = tuple(
        owner
        for task in tasks
        for owner in getattr(task, "owned_files", ())
    )
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
            raise BaselineError("baseline entry schema 不合法")
        paths = [entry["path"]]
        previous = entry.get("previous_path")
        if isinstance(previous, str):
            paths.append(previous)
        overlaps.extend(
            path
            for path in paths
            if any(_owner_covers_path(owner, path) for owner in owners)
        )
    if overlaps:
        raise BaselineError(
            "预存 dirty baseline 与 task owner 重叠："
            + ", ".join(sorted(set(overlaps)))
        )


def capture(repo_raw: Path, plan_raw: Path, output: Path) -> dict[str, object]:
    repo = canonical_repo_root(repo_raw)
    plan = plan_raw if plan_raw.is_absolute() else repo / plan_raw
    plan = plan.resolve()
    try:
        relative = plan.relative_to(repo).as_posix()
    except ValueError as error:
        raise BaselineError("plan 必须位于 repo 内") from error
    entries = capture_entries(repo)
    if output.exists():
        return validate_existing(repo, plan, output)
    if any(entry.path == relative for entry in entries):
        raise BaselineError(
            "control plan 在 execution start 前必须已落盘且工作树干净"
        )
    payload: dict[str, object] = {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "repo_root": str(repo),
        "plan": relative,
        "execution_start_head": head(repo),
        "entries": [entry_to_dict(entry) for entry in entries],
    }
    if not atomic_create_json(output, payload):
        return capture(repo, plan, output)
    return payload


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        print(stable_json(capture(args.repo, args.plan, args.output)), end="")
        return 0
    except (OSError, BaselineError, WorkspaceManifestError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
