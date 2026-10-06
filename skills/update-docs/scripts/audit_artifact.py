"""Model layer for the audit.json artifact: path normalization, construction, fail-closed validation, status recording, and PR rendering.

It handles only the artifact itself: no Git, no mappers, no file I/O. Those belong to the docs_audit.py CLI layer.
The split into two modules keeps the project's 400-line module boundary; here the semantic boundary and the line-count boundary coincide.
"""

from pathlib import PurePosixPath
from typing import Any, Sequence


SCHEMA_VERSION = 1
PENDING_STATUS = "pending"
FINAL_STATUSES = {"created", "updated", "verified-current"}
PR_STATUS = {
    "created": "created",
    "updated": "updated",
    "verified-current": "verified current",
}


def repository_path(value: str, *, field: str) -> str:
    # Backslashes must be rejected on the spot: PurePosixPath treats "a\\b\\c.md" as a **single** path
    # segment with depth 0, so a module document ranks alongside the repository-root document and the
    # "deepest to shallowest" order silently collapses on Windows. Nothing errors; the umbrella document
    # is simply audited before the module documents it references. The contract is repository-relative
    # POSIX paths, so fail closed right here.
    if "\\" in value:
        raise ValueError(f"{field} must use POSIX separators, got a backslash path: {value!r}")
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
        path = repository_path(value, field=field)
        if path in seen:
            raise ValueError(f"duplicate {field}: {path}")
        seen.add(path)
        normalized.append(path)
    return normalized


def _target_depth(path: str) -> int:
    """Depth of the directory that owns the target document. An AGENTS.md always sits in the directory it describes."""
    return len(PurePosixPath(path).parts) - 1


def merge_mapper_targets(target_lists: Sequence[Sequence[str]]) -> list[str]:
    merged: list[str] = []
    seen: set[str] = set()
    for targets in target_lists:
        for target in targets:
            path = repository_path(target, field="target")
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
        "targets": [{"path": path, "status": PENDING_STATUS, "reason": ""} for path in targets],
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
    if not isinstance(changed_leaves, list) or not all(isinstance(path, str) for path in changed_leaves):
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
        path = repository_path(path_value, field="target")
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
    if not isinstance(verification, list) or not all(isinstance(item, str) and item.strip() for item in verification):
        raise ValueError("verification must be an array of non-empty strings")
    if not isinstance(exclusions, list) or not all(isinstance(item, str) and item.strip() for item in exclusions):
        raise ValueError("exclusions must be an array of non-empty strings")
    complete = audit.get("complete")
    if not isinstance(complete, bool):
        raise ValueError("complete must be a boolean")
    if require_complete and not complete:
        raise ValueError("audit is not finalized")
    if require_complete and targets and not verification:
        raise ValueError("completed audit requires verification evidence")


def record_target(audit: dict[str, Any], *, path: str, status: str, reason: str) -> None:
    validate_audit(audit, require_complete=False)
    if audit["complete"]:
        raise ValueError("cannot modify a finalized audit")
    if status not in FINAL_STATUSES:
        raise ValueError(f"unknown status: {status!r}")
    if not reason.strip():
        raise ValueError("resolved target requires a reason")
    normalized_path = repository_path(path, field="target")
    for target in audit["targets"]:
        if target["path"] == normalized_path:
            target["status"] = status
            target["reason"] = reason.strip()
            return
    raise ValueError(f"unknown target: {normalized_path}")


def finalize_audit(audit: dict[str, Any], *, verification: Sequence[str], exclusions: Sequence[str]) -> None:
    validate_audit(audit, require_complete=False)
    pending = [target["path"] for target in audit["targets"] if target["status"] == PENDING_STATUS]
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
        lines.extend(f"- [x] `{target['path']}` — {PR_STATUS[target['status']]}" for target in audit["targets"])
    return "\n".join(lines)
