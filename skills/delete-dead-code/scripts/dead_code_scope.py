#!/usr/bin/env python3
"""Mechanical layer of delete-dead-code: PR module scope, batch planning, duplicate-candidate discovery, and
artifact verification.

The one truly dangerous failure of this skill is not a missed deletion but **deleting outside the PR scope**. Once
the scope is left to natural-language constraints, an agent carrying whole-repository audit conclusions drifts
outward. Making scope derivation, batching, and artifact verification deterministic scripts is what turns
out-of-scope edits and self-contradictions into nonzero exits instead of an ignored reminder.

In PR mode each top-level subtree (`src/`, `scripts/`, `tests/`, `docs/`, `skills/`, `vendor/`) is one unit;
loose top-level files have a separate repository-root unit. Use explicit directories for finer boundaries.

The scope has two mutually exclusive sources: by default the modules this PR touches, derived from `base...HEAD`;
with `--directories`, dead code is searched in the given directories instead (for explicit requests like "sweep
these trees"). Both modes share the same module expansion, batching, and artifact verification; explicit
directories do not "relax the scope", they only swap in another equally mechanical scope source.

The two tracks share scope and artifact verification, but differ in criteria and artifact shape (see guide.md):
  the dead-code track (`Track: dead`) fans out per module and only deletes;
  the duplicate track (`Track: duplicate`) discovers candidates across the whole scope and consolidates per owner set.

A unit is the boundary of a **write set** and also of worktree state: preflight, verify, and review look only at
the units this invocation owns and never read the whole tree. Units are pairwise disjoint, so other units'
in-flight changes do not belong to this invocation, and several invocations can run in parallel in one shared
worktree (guide.md "Why share one worktree").

Subcommands:
  scope   --repo (--base B | --directories D…)            → JSON: modules, file counts, directory maps, excludes
  preflight --repo (--base B | --directories D…) [--unit U…] → feature branch + no uncommitted change in own units
  batches --repo (--base B | --directories D…) [--max-parallel N] → JSON: parallel batches balanced by file count
  clones  --repo (--base B | --directories D…) [--min-lines N] [--include-tests] [--limit N]
                                                          → JSON: text-clone candidates across the whole scope
  verify  --repo (--base B | --directories D…) --artifact [--unit U…] → scope check + structural consistency check
  review  --repo (--base B | --directories D…) --artifact… [--unit U…] → consolidations and assertion rewrites the
                                                          main agent must review
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from dead_code_artifacts import (
    consolidations,
    handoffs,
    rewritten_assertions,
    verify_artifact,
)
from dead_code_clones import (
    DEFAULT_CLONE_LIMIT,
    DEFAULT_CLONE_MIN_LINES,
    clone_candidates,
    scope_files,
)
from dead_code_units import (
    DEFAULT_MAX_PARALLEL,
    REPOSITORY_MODULE,
    DeadCodeScopeError,
    Module,
    _git,
    dirty_paths,
    module_directories,
    module_of,
    plan_batches,
    preflight,
    scope,
    select_units,
    tracked_paths,
    unit_pathspecs,
)


# Tests and callers import from this entry point; the implementation is split into scope / artifact / clone layers
# in sibling modules.
__all__ = [
    "REPOSITORY_MODULE",
    "DeadCodeScopeError",
    "Module",
    "_git",
    "clone_candidates",
    "consolidations",
    "dirty_paths",
    "handoffs",
    "module_directories",
    "module_of",
    "plan_batches",
    "preflight",
    "rewritten_assertions",
    "scope",
    "scope_files",
    "select_units",
    "tracked_paths",
    "unit_pathspecs",
    "verify_artifact",
]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("scope", "preflight", "batches", "clones", "verify", "review"):
        child = sub.add_parser(name)
        child.add_argument("--repo", required=True, type=Path)
        source = child.add_mutually_exclusive_group(required=True)
        source.add_argument("--base")
        source.add_argument(
            "--directories",
            nargs="+",
            metavar="DIR",
            help="Search these directories for dead code instead of deriving the PR scope from base...HEAD.",
        )
        if name == "batches":
            child.add_argument("--max-parallel", type=int, default=DEFAULT_MAX_PARALLEL)
        if name == "clones":
            child.add_argument("--min-lines", type=int, default=DEFAULT_CLONE_MIN_LINES)
            child.add_argument(
                "--include-tests",
                action="store_true",
                help="Include tests and fixtures as candidates; by default only production code is considered.",
            )
            child.add_argument("--limit", type=int, default=DEFAULT_CLONE_LIMIT)
        if name in ("preflight", "verify", "review"):
            child.add_argument(
                "--unit",
                nargs="+",
                metavar="UNIT",
                help="Only these units (unit names from the scope output); defaults to every unit in scope.",
            )
        if name == "verify":
            child.add_argument("--artifact", required=True, type=Path)
        if name == "review":
            child.add_argument("--artifact", required=True, nargs="+", type=Path)
    arguments = parser.parse_args(argv)

    try:
        if arguments.command == "scope":
            modules = scope(arguments.repo, arguments.base, arguments.directories)
            print(json.dumps([m.as_json() for m in modules], ensure_ascii=False, indent=2))
            return 0
        if arguments.command == "preflight":
            problems = preflight(arguments.repo, arguments.base, arguments.directories, arguments.unit)
            for problem in problems:
                print(problem, file=sys.stderr)
            if problems:
                return 1
            print("Preflight holds: feature branch, no uncommitted change in this invocation's units")
            return 0
        if arguments.command == "batches":
            modules = scope(arguments.repo, arguments.base, arguments.directories)
            batches = plan_batches(modules, arguments.max_parallel)
            print(
                json.dumps(
                    {
                        "roundCount": len(batches),
                        "rounds": [[m.as_json() for m in batch] for batch in batches],
                    },
                    ensure_ascii=False,
                    indent=2,
                )
            )
            return 0
        if arguments.command == "clones":
            if arguments.limit < 1:
                raise DeadCodeScopeError(f"--limit must be >= 1, got {arguments.limit}")
            files = scope_files(arguments.repo, arguments.base, arguments.directories)
            result = clone_candidates(arguments.repo, files, arguments.min_lines, arguments.include_tests)
            candidates = result["candidates"]
            assert isinstance(candidates, list)
            # Truncation must be written to the output: a silent truncation reads as "everything was scanned".
            result["truncated"] = max(0, len(candidates) - arguments.limit)
            result["candidates"] = candidates[: arguments.limit]
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 0
        if arguments.command == "review":
            items = consolidations(list(arguments.artifact))
            units = select_units(scope(arguments.repo, arguments.base, arguments.directories), arguments.unit)
            rewritten = rewritten_assertions(arguments.repo, units)
            print(f"CONSOLIDATED pending review: {len(items)}")
            for artifact, anchor, line in items:
                print(f"  [{artifact.name}] {anchor}\n      {line}")
            print(f"Rewritten test assertions: {sum(count for _, count in rewritten)} line(s)")
            for path, count in rewritten:
                print(f"  {path} (+{count})")
            if items or rewritten:
                print(
                    "\nThe main agent must walk every item through the SKILL.md REVIEW stage: "
                    "read back from production call sites to confirm the consolidation direction; every flipped "
                    "assertion needs an \"updated to production facts\" explanation.",
                    file=sys.stderr,
                )
                return 1
            print("Nothing to review")
            return 0
        problems = verify_artifact(
            arguments.repo,
            arguments.base,
            arguments.artifact,
            arguments.directories,
            arguments.unit,
        )
        if problems:
            for problem in problems:
                print(problem, file=sys.stderr)
            return 1
        pending = handoffs(arguments.artifact)
        print("Artifact verified" + (f"; {len(pending)} HANDOFF(s) pending pairing" if pending else ""))
        return 0
    except DeadCodeScopeError as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
