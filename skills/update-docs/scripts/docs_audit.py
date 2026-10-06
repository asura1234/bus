#!/usr/bin/env python3
"""Mechanical CLI for update-docs: run mappers from changed Git leaves to build an audit, and forward status recording/finalization/rendering.

The construction and validation rules for audit.json itself live in audit_artifact.py in the same directory.

There is one documentation target family: `AGENTS.md`. In Bus, `skills/AGENTS.md` owns workflow
architecture, and vendored `AGENTS.md` files keep their upstream subtree contracts. The default Bus
mapper (`bus_documentation_targets`) maps changed leaves onto that actually-existing surface; Bus
intentionally has no repository-root AGENTS.md.

The mapper remains a repeatable argument: it is the interface between this script and a gate, not
an inference about "how many kinds of documents exist". The outputs of multiple mappers are merged,
de-duplicated, and reordered here by owning-directory depth from deepest to shallowest into a single
audit.json. The executing agent performs the semantic audit and editing; this script only proves
structure.
"""

import argparse
import json
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any, Sequence

from audit_artifact import (
    build_audit,
    finalize_audit,
    merge_mapper_targets,
    record_target,
    render_pr_section,
    repository_path,
    validate_audit,
)


def _run_lines(command: Sequence[str], *, cwd: Path) -> list[str]:
    result = subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True)
    return [line for line in result.stdout.splitlines() if line]


def _changed_paths(repo: Path, merge_base: str) -> list[str]:
    commands = [
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=ACDMRTUXB",
            f"{merge_base}...HEAD",
        ],
        ["git", "diff", "--name-only", "--diff-filter=ACDMRTUXB"],
        ["git", "diff", "--cached", "--name-only", "--diff-filter=ACDMRTUXB"],
        ["git", "ls-files", "--others", "--exclude-standard"],
    ]
    changed: set[str] = set()
    for command in commands:
        changed.update(_run_lines(command, cwd=repo))
    return sorted(repository_path(path, field="changed leaf") for path in changed)


def bus_documentation_targets(repo: Path, changed_paths: Sequence[str]) -> list[str]:
    """Map Bus leaves to the repository's actual AGENTS.md SOT surface."""

    targets: set[str] = set()
    for changed in changed_paths:
        if changed.startswith(("skills/", "docs/guides/", "docs/templates/", "cli_extensions/")) and (
            changed != "skills/AGENTS.md"
        ):
            targets.add("skills/AGENTS.md")
        path = PurePosixPath(changed).parent
        while path.as_posix() not in {".", ""}:
            candidate = path / "AGENTS.md"
            if (repo / candidate).is_file():
                targets.add(candidate.as_posix())
            path = path.parent
    return merge_mapper_targets([sorted(targets)])


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(f"{json.dumps(value, ensure_ascii=False, indent=2)}\n", encoding="utf-8")


def _load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("audit root must be an object")
    return value


def _all_module_targets(repo: Path, mapper: Path | None) -> list[str]:
    """Whole-repo target set: every existing AGENTS.md, plus each module an explicit mapper discovers (which may not exist yet)."""
    targets: set[str] = set()
    if mapper is not None:
        for line in _run_lines(["node", str(mapper), "--repo", str(repo), "--list-modules"], cwd=repo):
            _kind, _, name = line.partition("\t")
            if name:
                targets.add(f"{name}/AGENTS.md")
    for line in _run_lines(["git", "ls-files", "--", "*AGENTS.md", "AGENTS.md"], cwd=repo):
        targets.add(line)
    return sorted(targets)


def _prepare(args: argparse.Namespace) -> str:
    repo = Path(args.repo).resolve()
    actual_root = Path(_run_lines(["git", "rev-parse", "--show-toplevel"], cwd=repo)[0]).resolve()
    if actual_root != repo:
        raise ValueError(f"--repo must be the Git root: {repo}")
    merge_base = _run_lines(["git", "merge-base", args.base, "HEAD"], cwd=repo)[0]
    changed = _changed_paths(repo, merge_base)
    if args.output_dir is None:
        timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        output_dir = repo / "temp" / "update-docs" / timestamp
    else:
        output_dir = Path(args.output_dir).resolve()
    output_dir.mkdir(parents=True, exist_ok=False)
    changed_file = output_dir / "changed-files.txt"
    changed_file.write_text("".join(f"{path}\n" for path in changed), encoding="utf-8")

    mapper_paths = [Path(mapper).resolve() for mapper in args.mapper or []]
    if args.all_modules:
        # Whole-repo scope: targets do not come from the diff walk but from "every existing
        # AGENTS.md (plus every module an explicit mapper discovers)". Used for whole-repo audits
        # unrelated to this change, such as template migrations or full backfills.
        target_lists = [_all_module_targets(repo, mapper_paths[0] if mapper_paths else None)]
    elif not mapper_paths:
        target_lists = [bus_documentation_targets(repo, changed)]
    else:
        target_lists = [
            _run_lines(
                [
                    "node",
                    str(mapper),
                    "--repo",
                    str(repo),
                    "--list-affected",
                    "--changed-files",
                    str(changed_file),
                ],
                cwd=repo,
            )
            for mapper in mapper_paths
        ]
    targets = merge_mapper_targets(target_lists)
    (output_dir / "targets.txt").write_text("".join(f"{path}\n" for path in targets), encoding="utf-8")
    audit = build_audit(
        base=args.base,
        merge_base=merge_base,
        changed_paths=changed,
        target_paths=targets,
    )
    audit_path = output_dir / "audit.json"
    _write_json(audit_path, audit)
    return str(audit_path)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Prepare and validate AGENTS.md documentation audits")
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare")
    prepare.add_argument("--repo", required=True)
    prepare.add_argument("--base", default="origin/master")
    prepare.add_argument(
        "--mapper",
        action="append",
        default=None,
        help="recursive target mapper script, repeatable; defaults to the built-in Bus AGENTS.md mapper",
    )
    prepare.add_argument("--output-dir")
    prepare.add_argument(
        "--all-modules",
        action="store_true",
        help="ignore the diff walk and list every existing AGENTS.md (plus modules an explicit mapper discovers) as targets",
    )

    record = subparsers.add_parser("record")
    record.add_argument("--audit", required=True)
    record.add_argument("--path", required=True)
    record.add_argument("--status", required=True)
    record.add_argument("--reason", required=True)

    finalize = subparsers.add_parser("finalize")
    finalize.add_argument("--audit", required=True)
    finalize.add_argument("--verification", action="append", default=[])
    finalize.add_argument("--exclusion", action="append", default=[])

    render = subparsers.add_parser("render-pr")
    render.add_argument("--audit", required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "prepare":
            print(_prepare(args))
        elif args.command == "record":
            audit_path = Path(args.audit).resolve()
            audit = _load_json(audit_path)
            record_target(
                audit,
                path=args.path,
                status=args.status,
                reason=args.reason,
            )
            _write_json(audit_path, audit)
        elif args.command == "finalize":
            audit_path = Path(args.audit).resolve()
            audit = _load_json(audit_path)
            finalize_audit(
                audit,
                verification=args.verification,
                exclusions=args.exclusion,
            )
            validate_audit(audit, require_complete=True)
            _write_json(audit_path, audit)
        elif args.command == "render-pr":
            print(render_pr_section(_load_json(Path(args.audit).resolve())))
        else:
            raise ValueError(f"unknown command: {args.command}")
    except (
        ValueError,
        OSError,
        subprocess.CalledProcessError,
        json.JSONDecodeError,
    ) as error:
        print(f"docs audit failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
