#!/usr/bin/env python3
"""consumer fallout 的 module graph 索引与高置信摘要。"""

from __future__ import annotations

import posixpath
import sys
from pathlib import Path, PurePosixPath
from typing import Any

from task_graph_validation import _owner_covers_path


SOURCE_SUFFIXES = frozenset(
    {
        ".c",
        ".cc",
        ".cpp",
        ".h",
        ".hpp",
        ".java",
        ".js",
        ".jsx",
        ".kt",
        ".kts",
        ".m",
        ".mm",
        ".py",
        ".rs",
        ".swift",
        ".ts",
        ".tsx",
    }
)
TEST_MARKERS = (".test.", ".spec.", "_test.", "_tests.", "Tests.", "Test.")
TEST_DIRECTORY_NAMES = frozenset(
    {"__mocks__", "__test-mocks__", "__tests__", "tests"}
)
TEST_REASON = "same-name-test"
DIRECT_REASON = "direct-module-reference"
DEFAULT_ACTIONABLE_LIMIT = 8


def source_stem(path: str) -> str:
    name = PurePosixPath(path).name
    while Path(name).suffix in SOURCE_SUFFIXES:
        name = Path(name).stem
    return name


def _without_source_suffix(path: PurePosixPath) -> PurePosixPath:
    while Path(path.name).suffix in SOURCE_SUFFIXES:
        path = path.with_name(Path(path.name).stem)
    return path


def source_module_paths(source: str) -> set[str]:
    source_path = _without_source_suffix(PurePosixPath(source))
    paths = {source_path.as_posix()}
    if source_path.name in {"index", "lib", "main", "mod"}:
        paths.add(source_path.parent.as_posix())
    return paths


def specifier_targets(importer: str, specifier: str) -> set[str]:
    if "::" in specifier:
        parts = [part for part in specifier.split("::") if part]
        importer_parent = PurePosixPath(importer).parent
        if parts and parts[0] == "crate":
            base = PurePosixPath("src")
            parts = parts[1:]
        elif parts and parts[0] in {"self", "super"}:
            base = importer_parent
            parts = parts[1:]
            while parts and parts[0] == "super":
                base = base.parent
                parts = parts[1:]
        else:
            base = importer_parent

        targets: set[str] = set()
        current = base
        for part in parts:
            current /= part
            targets.add(current.as_posix())
        return targets
    if specifier.startswith("."):
        target = posixpath.normpath(
            posixpath.join(PurePosixPath(importer).parent.as_posix(), specifier)
        )
        return {_without_source_suffix(PurePosixPath(target)).as_posix()}
    target = _without_source_suffix(PurePosixPath(specifier))
    return {target.as_posix(), target.name}


def references_source(importer: str, specifier: str, source: str) -> bool:
    source_paths = source_module_paths(source)
    targets = specifier_targets(importer, specifier)
    if specifier.startswith(".") or "::" in specifier:
        return not source_paths.isdisjoint(targets)
    source_names = {PurePosixPath(path).name for path in source_paths}
    return not source_names.isdisjoint(targets)


def is_test(path: str) -> bool:
    name = PurePosixPath(path).name
    return not TEST_DIRECTORY_NAMES.isdisjoint(PurePosixPath(path).parts) or any(
        marker in name for marker in TEST_MARKERS
    )


def test_subject_stem(path: str) -> str | None:
    name = PurePosixPath(path).name
    positions = [name.find(marker) for marker in TEST_MARKERS if marker in name]
    return name[: min(positions)] if positions else None


def task_ids_covering(tasks: list[Any], path: str) -> list[int]:
    return [
        task.id
        for task in tasks
        if any(_owner_covers_path(owner, path) for owner in task.owned_files)
    ]


def candidate_record(
    path: str,
    reasons: set[str],
    declared_paths: set[str],
    tasks: list[Any],
) -> dict[str, Any]:
    owner_task_ids = task_ids_covering(tasks, path)
    return {
        "path": path,
        "reasons": sorted(reasons),
        "declared_in_file_contract": path in declared_paths,
        "owner_task_ids": owner_task_ids,
        "covered_by_owner": bool(owner_task_ids),
    }


def _high_confidence_item(
    *,
    source: str,
    candidate_path: str,
    relation: str,
    chain: list[str],
    evidence: list[str],
    declared_paths: set[str],
    tasks: list[Any],
) -> dict[str, Any]:
    source_task_ids = task_ids_covering(tasks, source)
    owner_task_ids = task_ids_covering(tasks, candidate_path)
    declared = candidate_path in declared_paths
    if owner_task_ids and declared:
        resolution = "requires-gate-review-or-explicit-exclusion"
    elif owner_task_ids:
        resolution = "requires-file-contract-or-explicit-exclusion"
    else:
        resolution = "requires-owner-or-explicit-exclusion"
    return {
        "source": source,
        "candidate": candidate_path,
        "relation": relation,
        "chain": chain,
        "evidence": sorted(evidence),
        "source_task_ids": source_task_ids,
        "candidate_owner_task_ids": owner_task_ids,
        "declared_in_file_contract": declared,
        "resolution": resolution,
    }


def high_confidence_summary(
    entries: list[dict[str, Any]],
    specifiers_by_path: dict[str, tuple[str, ...]],
    tests_by_stem: dict[str, tuple[str, ...]],
    declared_paths: set[str],
    tasks: list[Any],
) -> dict[str, Any]:
    items: dict[tuple[str, str, str, tuple[str, ...]], dict[str, Any]] = {}
    for entry in entries:
        source = entry["source"]
        for candidate in entry["candidates"]:
            reasons = set(candidate["reasons"])
            if reasons == {DIRECT_REASON, TEST_REASON}:
                item = _high_confidence_item(
                    source=source,
                    candidate_path=candidate["path"],
                    relation="direct-reference-and-same-name-test",
                    chain=[source, candidate["path"]],
                    evidence=[DIRECT_REASON, TEST_REASON],
                    declared_paths=declared_paths,
                    tasks=tasks,
                )
                key = (source, candidate["path"], item["relation"], tuple(item["chain"]))
                items[key] = item

        direct_consumers = [
            candidate["path"]
            for candidate in entry["candidates"]
            if DIRECT_REASON in candidate["reasons"] and not is_test(candidate["path"])
        ]
        for consumer in direct_consumers:
            for test_path in tests_by_stem.get(source_stem(consumer), ()):
                evidence = [TEST_REASON]
                if any(
                    references_source(test_path, specifier, consumer)
                    for specifier in specifiers_by_path[test_path]
                ):
                    evidence.append(DIRECT_REASON)
                item = _high_confidence_item(
                    source=source,
                    candidate_path=test_path,
                    relation="one-hop-consumer-test",
                    chain=[source, consumer, test_path],
                    evidence=evidence,
                    declared_paths=declared_paths,
                    tasks=tasks,
                )
                key = (source, test_path, item["relation"], tuple(item["chain"]))
                items[key] = item

    by_task: dict[str, list[dict[str, Any]]] = {}
    for item in sorted(
        items.values(),
        key=lambda value: (
            value["source_task_ids"] or [sys.maxsize],
            value["source"],
            value["candidate"],
            value["relation"],
        ),
    ):
        for task_id in item["source_task_ids"] or ["unassigned"]:
            by_task.setdefault(str(task_id), []).append(item)
    unresolved = len(items)
    return {
        "high_confidence_count": len(items),
        "unresolved_count": unresolved,
        "by_task": by_task,
    }


def actionable_summary(
    summary: dict[str, Any],
    *,
    limit_per_task: int = DEFAULT_ACTIONABLE_LIMIT,
) -> dict[str, Any]:
    by_task: dict[str, dict[str, Any]] = {}
    for task_id, items in summary["by_task"].items():
        unresolved = sorted(
            items,
            key=lambda item: (
                item["relation"] != "direct-reference-and-same-name-test",
                item["chain"],
            ),
        )
        selected = unresolved[:limit_per_task]
        by_task[task_id] = {
            "high_confidence_count": len(items),
            "resolved_count": len(items) - len(unresolved),
            "unresolved_count": len(unresolved),
            "omitted_unresolved_count": max(0, len(unresolved) - len(selected)),
            "items": [
                {
                    "relation": item["relation"],
                    "chain": item["chain"],
                    "resolution": item["resolution"],
                }
                for item in selected
            ],
        }
    return {
        "high_confidence_count": summary["high_confidence_count"],
        "unresolved_count": summary["unresolved_count"],
        "limit_per_task": limit_per_task,
        "by_task": by_task,
    }


def candidate_reasons_by_source(
    sources: tuple[str, ...],
    specifiers_by_path: dict[str, tuple[str, ...]],
    tests_by_stem: dict[str, tuple[str, ...]],
) -> dict[str, dict[str, set[str]]]:
    candidates = {source: {} for source in sources}
    source_lookup: dict[str, set[str]] = {}
    for source in sources:
        for module_path in source_module_paths(source):
            source_lookup.setdefault(module_path, set()).add(source)
            source_lookup.setdefault(PurePosixPath(module_path).name, set()).add(source)
        for test_path in tests_by_stem.get(source_stem(source), ()):
            candidates[source].setdefault(test_path, set()).add(TEST_REASON)

    for importer, specifiers in specifiers_by_path.items():
        for specifier in specifiers:
            for target in specifier_targets(importer, specifier):
                for source in source_lookup.get(target, ()):
                    if importer != source:
                        candidates[source].setdefault(importer, set()).add(
                            DIRECT_REASON
                        )
    return candidates
