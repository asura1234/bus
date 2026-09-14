#!/usr/bin/env python3
"""update-docs 的机械层：文档审计 artifact 的准备、状态记录、fail-closed 校验与渲染。

文档目标只有一类：`AGENTS.md`——模块根的那份是 as-built 模块 SOT（必备 section 见
docs/templates/module-agents-template.md），伞级目录那份描述子树划分与入口。两者由
scripts/lint/check-module-docs.mjs 用同一套模块发现规则映射出来。

mapper 仍是可重复参数：它是本脚本与门禁之间的接口，不是「有几类文档」的推论。多个 mapper
的输出在这里合并去重并按目标所属目录深度从深到浅重排，形成单一 audit.json。语义审计与编辑
由执行 agent 完成，本脚本只证明结构。
"""

import argparse
import json
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any, Sequence


SCHEMA_VERSION = 1
PENDING_STATUS = "pending"
FINAL_STATUSES = {"created", "updated", "verified-current"}
PR_STATUS = {
    "created": "created",
    "updated": "updated",
    "verified-current": "verified current",
}


def _repository_path(value: str, *, field: str) -> str:
    # 反斜杠必须当场拒绝：PurePosixPath 会把 "a\\b\\c.md" 当成**单个**路径段，深度算成 0，
    # 于是模块文档与仓库根文档并列，"从深到浅" 的顺序在 Windows 上静默塌掉——不报错，
    # 只是伞级文档先于它引用的模块文档被审计。契约是仓库相对的 POSIX 路径，就地 fail-closed。
    if "\\" in value:
        raise ValueError(
            f"{field} must use POSIX separators, got a backslash path: {value!r}"
        )
    path = PurePosixPath(value)
    if value == "" or path.is_absolute() or ".." in path.parts:
        raise ValueError(f"{field} must be a repository-relative path: {value!r}")
    normalized = path.as_posix()
    if normalized == ".":
        raise ValueError(f"{field} must be a repository-relative path: {value!r}")
    return normalized


def _unique_paths(values: Sequence[str], *, field: str) -> list[str]:
    normalized: list[str] = []
    seen: set[str] = set()
    for value in values:
        path = _repository_path(value, field=field)
        if path in seen:
            raise ValueError(f"duplicate {field}: {path}")
        seen.add(path)
        normalized.append(path)
    return normalized


def _target_depth(path: str) -> int:
    """目标文档所属目录的深度。AGENTS.md 始终躺在它描述的那个目录里。"""
    return len(PurePosixPath(path).parts) - 1


def merge_mapper_targets(target_lists: Sequence[Sequence[str]]) -> list[str]:
    merged: list[str] = []
    seen: set[str] = set()
    for targets in target_lists:
        for target in targets:
            path = _repository_path(target, field="target")
            if path in seen:
                continue
            seen.add(path)
            merged.append(path)
    return sorted(merged, key=lambda path: (-_target_depth(path), path))


def build_audit(
    *,
    base: str,
    merge_base: str,
    changed_paths: Sequence[str],
    target_paths: Sequence[str],
) -> dict[str, Any]:
    changed = _unique_paths(changed_paths, field="changed leaf")
    targets = _unique_paths(target_paths, field="target")
    return {
        "schemaVersion": SCHEMA_VERSION,
        "complete": False,
        "base": base,
        "mergeBase": merge_base,
        "changedLeaves": changed,
        "targets": [
            {"path": path, "status": PENDING_STATUS, "reason": ""} for path in targets
        ],
        "verification": [],
        "exclusions": [],
    }


def validate_audit(audit: dict[str, Any], *, require_complete: bool) -> None:
    if audit.get("schemaVersion") != SCHEMA_VERSION:
        raise ValueError("unsupported or missing schemaVersion")
    if not isinstance(audit.get("base"), str) or not audit["base"]:
        raise ValueError("base must be a non-empty string")
    if not isinstance(audit.get("mergeBase"), str) or not audit["mergeBase"]:
        raise ValueError("mergeBase must be a non-empty string")

    changed_leaves = audit.get("changedLeaves")
    if not isinstance(changed_leaves, list) or not all(
        isinstance(path, str) for path in changed_leaves
    ):
        raise ValueError("changedLeaves must be a path array")
    _unique_paths(changed_leaves, field="changed leaf")

    targets = audit.get("targets")
    if not isinstance(targets, list):
        raise ValueError("targets must be an array")
    seen: set[str] = set()
    for target in targets:
        if not isinstance(target, dict):
            raise ValueError("target must be an object")
        path_value = target.get("path")
        if not isinstance(path_value, str):
            raise ValueError("target path must be a string")
        path = _repository_path(path_value, field="target")
        if path in seen:
            raise ValueError(f"duplicate target: {path}")
        seen.add(path)
        status = target.get("status")
        if status not in FINAL_STATUSES | {PENDING_STATUS}:
            raise ValueError(f"unknown status: {status!r}")
        reason = target.get("reason")
        if not isinstance(reason, str):
            raise ValueError(f"target reason must be a string: {path}")
        if status in FINAL_STATUSES and not reason.strip():
            raise ValueError(f"resolved target requires a reason: {path}")
        if require_complete and status == PENDING_STATUS:
            raise ValueError(f"pending target: {path}")

    verification = audit.get("verification")
    exclusions = audit.get("exclusions")
    if not isinstance(verification, list) or not all(
        isinstance(item, str) and item.strip() for item in verification
    ):
        raise ValueError("verification must be an array of non-empty strings")
    if not isinstance(exclusions, list) or not all(
        isinstance(item, str) and item.strip() for item in exclusions
    ):
        raise ValueError("exclusions must be an array of non-empty strings")
    complete = audit.get("complete")
    if not isinstance(complete, bool):
        raise ValueError("complete must be a boolean")
    if require_complete and not complete:
        raise ValueError("audit is not finalized")
    if require_complete and targets and not verification:
        raise ValueError("completed audit requires verification evidence")


def record_target(
    audit: dict[str, Any], *, path: str, status: str, reason: str
) -> None:
    validate_audit(audit, require_complete=False)
    if audit["complete"]:
        raise ValueError("cannot modify a finalized audit")
    if status not in FINAL_STATUSES:
        raise ValueError(f"unknown status: {status!r}")
    if not reason.strip():
        raise ValueError("resolved target requires a reason")
    normalized_path = _repository_path(path, field="target")
    for target in audit["targets"]:
        if target["path"] == normalized_path:
            target["status"] = status
            target["reason"] = reason.strip()
            return
    raise ValueError(f"unknown target: {normalized_path}")


def finalize_audit(
    audit: dict[str, Any], *, verification: Sequence[str], exclusions: Sequence[str]
) -> None:
    validate_audit(audit, require_complete=False)
    pending = [
        target["path"]
        for target in audit["targets"]
        if target["status"] == PENDING_STATUS
    ]
    if pending:
        raise ValueError(f"pending target: {pending[0]}")
    normalized_verification = [item.strip() for item in verification if item.strip()]
    normalized_exclusions = [item.strip() for item in exclusions if item.strip()]
    if audit["targets"] and not normalized_verification:
        raise ValueError("completed audit requires verification evidence")
    audit["verification"] = normalized_verification
    audit["exclusions"] = normalized_exclusions
    audit["complete"] = True


def render_pr_section(audit: dict[str, Any]) -> str:
    validate_audit(audit, require_complete=True)
    lines = ["## 文档同步", ""]
    if not audit["targets"]:
        lines.append("- [x] No affected documentation targets")
    else:
        lines.extend(
            f"- [x] `{target['path']}` — {PR_STATUS[target['status']]}"
            for target in audit["targets"]
        )
    return "\n".join(lines)


def _run_lines(command: Sequence[str], *, cwd: Path) -> list[str]:
    result = subprocess.run(
        command, cwd=cwd, check=True, capture_output=True, text=True
    )
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
    return sorted(_repository_path(path, field="changed leaf") for path in changed)


def bus_documentation_targets(repo: Path, changed_paths: Sequence[str]) -> list[str]:
    """Map Bus leaves to the repository's actual AGENTS.md SOT surface."""

    targets: set[str] = set()
    for changed in changed_paths:
        if changed.startswith(
            ("skills/", "docs/guides/", "docs/templates/", "cli_extensions/")
        ) and changed != "skills/AGENTS.md":
            targets.add("skills/AGENTS.md")
        path = PurePosixPath(changed).parent
        while path.as_posix() not in {".", ""}:
            candidate = path / "AGENTS.md"
            if (repo / candidate).is_file():
                targets.add(candidate.as_posix())
            path = path.parent
    return sorted(targets, key=lambda path: (-_target_depth(path), path))


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(
        f"{json.dumps(value, ensure_ascii=False, indent=2)}\n", encoding="utf-8"
    )


def _load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("audit root must be an object")
    return value


def _prepare(args: argparse.Namespace) -> str:
    repo = Path(args.repo).resolve()
    actual_root = Path(
        _run_lines(["git", "rev-parse", "--show-toplevel"], cwd=repo)[0]
    ).resolve()
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

    if args.mapper is None:
        targets = bus_documentation_targets(repo, changed)
    else:
        mapper_paths = [Path(mapper).resolve() for mapper in args.mapper]
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
    (output_dir / "targets.txt").write_text(
        "".join(f"{path}\n" for path in targets), encoding="utf-8"
    )
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
    parser = argparse.ArgumentParser(
        description="Prepare and validate AGENTS.md documentation audits"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare")
    prepare.add_argument("--repo", required=True)
    prepare.add_argument("--base", default="origin/master")
    prepare.add_argument(
        "--mapper",
        action="append",
        default=None,
        help="递归目标 mapper 脚本，可重复；缺省使用模块文档门禁 check-module-docs.mjs",
    )
    prepare.add_argument("--output-dir")

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
