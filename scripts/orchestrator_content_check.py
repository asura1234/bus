#!/usr/bin/env python3
"""Objective static checks for the production room-orchestrator content bundle.

`build.rs` enforces manifest schema, digests, flat paths, UTF-8, and size caps. This checker
only proves the required entry set, workflow document structure, and the absence of
capability tokens removed from the closed prerequisite surface. It never interprets a
diagram, classifies wording, or executes a model.
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


REPO = Path(__file__).resolve().parents[1]
PRODUCTION = Path("src/bus/orchestrator/content/production")
MANIFEST = PRODUCTION / "manifest.json"
REQUIRED_SKILLS = ("create-workflow", "execute-workflow")
REQUIRED_REFERENCES = (
    "workflow-template",
    "sop-review-plan",
    "sop-review-pr",
    "sop-execute-plan",
)
WORKFLOW_TEMPLATE = "workflow-template"
BUILT_IN_SOPS = ("sop-review-plan", "sop-review-pr", "sop-execute-plan")
TEMPLATE_SECTIONS = (
    "Intent",
    "Inputs",
    "Participants and capabilities",
    "Room Brief boundary",
    "Current SOP source",
    "Success evidence",
    "Settlement evidence",
    "Failure signals",
    "Adaptation and recovery authority",
    "Resource leases",
    "Attempt ledger",
    "Human tasks",
    "Stop and escalation",
    "Revision history",
)
CANONICAL_SKILLS = (
    "create-plan",
    "review-plan",
    "execute-plan",
    "pr",
    "review-pr",
    "address-review-comments",
)
ROOM_BRIEF_GUIDE = Path("docs/guides/orchestrated-room-brief.md")
# The closed prerequisite surface deleted these queries, operations, and handles.
REMOVED_CAPABILITY_TOKENS = (
    "CreateAgent",
    "SteerActiveWork",
    "SuspendWork",
    "ResolveQueuedWork",
    "UpdateCoordination",
    "RetireParticipant",
    "ReplaceParticipant",
    "RequestHuman",
    "ReadCodebaseOutline",
    "ReadArtifact",
    "OutlineCursor",
)
FENCE_RE = re.compile(r"^\s*(`{3,}|~{3,})\s*(\S*)")
H2_RE = re.compile(r"^## (.+?)\s*$")


@dataclass(frozen=True)
class Diagnostic:
    path: str
    message: str


def snake_case(token: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", token).lower()


def _removed_token_pattern() -> re.Pattern[str]:
    names = sorted(
        {*REMOVED_CAPABILITY_TOKENS, *(snake_case(token) for token in REMOVED_CAPABILITY_TOKENS)},
        key=len,
        reverse=True,
    )
    return re.compile(r"(?<![A-Za-z0-9_])(" + "|".join(map(re.escape, names)) + r")(?![A-Za-z0-9_])")


def _load_manifest(repo: Path) -> dict | None:
    try:
        payload = json.loads((repo / MANIFEST).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    return payload if isinstance(payload, dict) else None


def _named_entries(manifest: dict, kind: str) -> dict[str, object]:
    entries = manifest.get(kind)
    if not isinstance(entries, list):
        return {}
    return {
        entry["name"]: entry.get("path")
        for entry in entries
        if isinstance(entry, dict) and isinstance(entry.get("name"), str)
    }


def _entry_file(repo: Path, manifest: dict | None, kind: str, name: str) -> Path | None:
    if manifest is None:
        return None
    path = _named_entries(manifest, kind).get(name)
    if not isinstance(path, str) or not _is_flat_markdown(path):
        return None
    return repo / PRODUCTION / path


def _is_flat_markdown(path: str) -> bool:
    pure = PurePosixPath(path)
    return len(pure.parts) == 1 and pure.suffix == ".md" and "\\" not in path


def _outside_fences(text: str) -> list[str]:
    lines: list[str] = []
    fence: str | None = None
    for line in text.splitlines():
        match = FENCE_RE.match(line)
        if match is not None:
            marker = match.group(1)
            if fence is None:
                fence = marker
            elif marker[0] == fence[0] and len(marker) >= len(fence):
                fence = None
            continue
        if fence is None:
            lines.append(line)
    return lines


def mermaid_flowchart_count(text: str) -> int:
    count = 0
    fence: str | None = None
    language = ""
    first_line: str | None = None
    for line in text.splitlines():
        match = FENCE_RE.match(line)
        if match is not None:
            marker = match.group(1)
            if fence is None:
                fence, language, first_line = marker, match.group(2), None
                continue
            if marker[0] == fence[0] and len(marker) >= len(fence):
                if language == "mermaid" and (first_line or "").startswith("flowchart"):
                    count += 1
                fence = None
                continue
        if fence is not None and first_line is None and line.strip():
            first_line = line.strip()
    return count


def validate_production_entry_set(repo: Path) -> list[Diagnostic]:
    manifest_path = MANIFEST.as_posix()
    manifest = _load_manifest(repo)
    if manifest is None:
        return [Diagnostic(manifest_path, "manifest is missing or not a JSON object")]
    diagnostics: list[Diagnostic] = []
    paths: list[tuple[str, object]] = []
    for slot in ("system", "agent"):
        entry = manifest.get(slot)
        if not isinstance(entry, dict):
            diagnostics.append(Diagnostic(manifest_path, f"missing required {slot} entry"))
            continue
        paths.append((slot, entry.get("path")))
    for kind, required, label in (
        ("skills", REQUIRED_SKILLS, "skill"),
        ("references", REQUIRED_REFERENCES, "reference"),
    ):
        entries = _named_entries(manifest, kind)
        for name in required:
            if name not in entries:
                diagnostics.append(Diagnostic(manifest_path, f"missing required {label} entry {name}"))
            else:
                paths.append((f"{label}/{name}", entries[name]))
    for label, path in paths:
        if not isinstance(path, str) or not _is_flat_markdown(path):
            diagnostics.append(Diagnostic(manifest_path, f"{label} path must be one flat .md file"))
        elif not (repo / PRODUCTION / path).is_file():
            diagnostics.append(Diagnostic(manifest_path, f"{label} file {path} is missing"))
    return diagnostics


def validate_workflow_template_structure(repo: Path) -> list[Diagnostic]:
    path = _entry_file(repo, _load_manifest(repo), "references", WORKFLOW_TEMPLATE)
    if path is None or not path.is_file():
        return []
    text = path.read_text(encoding="utf-8")
    relative = path.relative_to(repo).as_posix()
    headings = {match.group(1) for line in _outside_fences(text) if (match := H2_RE.fullmatch(line))}
    diagnostics = [
        Diagnostic(relative, f"missing required section `## {section}`")
        for section in TEMPLATE_SECTIONS
        if section not in headings
    ]
    count = mermaid_flowchart_count(text)
    if count != 1:
        diagnostics.append(Diagnostic(relative, f"expected exactly one Mermaid flowchart, found {count}"))
    return diagnostics


def validate_single_mermaid_flowchart_per_sop(repo: Path) -> list[Diagnostic]:
    manifest = _load_manifest(repo)
    diagnostics: list[Diagnostic] = []
    for name in BUILT_IN_SOPS:
        path = _entry_file(repo, manifest, "references", name)
        if path is None or not path.is_file():
            continue
        count = mermaid_flowchart_count(path.read_text(encoding="utf-8"))
        if count != 1:
            diagnostics.append(
                Diagnostic(
                    path.relative_to(repo).as_posix(),
                    f"expected exactly one Mermaid flowchart, found {count}",
                )
            )
    return diagnostics


def validate_no_removed_capability_tokens(repo: Path) -> list[Diagnostic]:
    targets = sorted((repo / PRODUCTION).glob("*.md"))
    targets += [repo / "skills" / name / "SKILL.md" for name in CANONICAL_SKILLS]
    targets.append(repo / ROOM_BRIEF_GUIDE)
    pattern = _removed_token_pattern()
    diagnostics: list[Diagnostic] = []
    for path in targets:
        relative = path.relative_to(repo).as_posix()
        if not path.is_file():
            diagnostics.append(Diagnostic(relative, "scanned file is missing"))
            continue
        found: list[str] = []
        for match in pattern.finditer(path.read_text(encoding="utf-8")):
            if match.group(1) not in found:
                found.append(match.group(1))
        diagnostics.extend(
            Diagnostic(relative, f"removed capability token `{token}`") for token in found
        )
    return diagnostics


def validate_repository(repo: Path) -> tuple[Diagnostic, ...]:
    return (
        *validate_production_entry_set(repo),
        *validate_workflow_template_structure(repo),
        *validate_single_mermaid_flowchart_per_sop(repo),
        *validate_no_removed_capability_tokens(repo),
    )


def main() -> int:
    diagnostics = validate_repository(REPO)
    for diagnostic in diagnostics:
        print(f"{diagnostic.path}: {diagnostic.message}")
    if diagnostics:
        return 1
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
