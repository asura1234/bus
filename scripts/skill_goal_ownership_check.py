#!/usr/bin/env python3
"""Mechanical Room Brief ownership checks for the canonical coding-agent skills.

The checks are objective: shared-guide references, the single consumer and lock writer,
branch markers, discovery links, entrypoint length, and severity labels. Semantic quality
of a skill's judgment stays with review.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from sanitize_review_severity import sanitize_review  # noqa: E402


REPO = SCRIPT_DIR.parent
CANONICAL_SKILLS = (
    "create-plan",
    "review-plan",
    "execute-plan",
    "pr",
    "review-pr",
    "address-review-comments",
)
GUIDE = "docs/guides/orchestrated-room-brief.md"
CONSUMER = "cli_extensions/room_assignment_context.py"
LOCK_WRITER = "pr_goal_context.py"
DISCOVERY_VARIABLES = (
    "BUS_BINARY",
    "BUS_TRUSTED_ASSIGNMENT_DIR",
    "BUS_TRUSTED_ASSIGNMENT_ENDPOINT",
    "BUS_TRUSTED_ASSIGNMENT_TOKEN",
)
GUIDE_REQUIRED_TEXT = (
    *DISCOVERY_VARIABLES,
    "NotInBusRoom",
    "verified",
    "--bus assignment verify --frame",
    CONSUMER,
    LOCK_WRITER,
)
GUIDE_REQUIRED_HEADINGS = (
    "## Authority",
    "## Branch mapping",
    "## Goal and Non-goals precedence",
    "## Participant handoff",
)
SKILL_REQUIRED_TEXT = (GUIDE, CONSUMER, "NotInBusRoom", "IF ORIGIN == verified")
ORCHESTRATOR_ONLY_SKILLS = ("create-workflow", "execute-workflow")
DISCOVERY_REGISTRIES = (".agents/skills", ".claude/skills")
TRUST_STORAGE_MARKERS = (".read-token", "active.json")
ENTRYPOINT_MAX_LINES = 250
LOCK_NAMES_RE = re.compile(r"\.locked-(?:goal|non-goals)")
WRITE_RE = re.compile(r"\bwrit(?:e|es|ing|ten)\b|写", re.IGNORECASE)


@dataclass(frozen=True, order=True)
class Violation:
    path: str
    message: str


def _helper_sources(repo: Path) -> list[Path]:
    roots = [repo / "cli_extensions", *sorted((repo / "skills").glob("*/scripts"))]
    return sorted(
        path
        for root in roots
        if root.is_dir()
        for path in root.glob("*.py")
        if not path.name.startswith("test_")
    )


def verify_shared_room_brief_contract(repo: Path) -> list[Violation]:
    violations: list[Violation] = []
    guide = repo / GUIDE
    if not guide.is_file():
        violations.append(Violation(GUIDE, "shared Room Brief guide is missing"))
    else:
        text = guide.read_text(encoding="utf-8")
        lines = set(text.splitlines())
        violations.extend(
            Violation(GUIDE, f"guide does not state `{required}`")
            for required in GUIDE_REQUIRED_TEXT
            if required not in text
        )
        violations.extend(
            Violation(GUIDE, f"guide lacks heading `{heading}`")
            for heading in GUIDE_REQUIRED_HEADINGS
            if heading not in lines
        )
    if not (repo / CONSUMER).is_file():
        violations.append(Violation(CONSUMER, "shared room assignment consumer is missing"))
    for path in _helper_sources(repo):
        relative = path.relative_to(repo).as_posix()
        text = path.read_text(encoding="utf-8")
        violations.extend(
            Violation(relative, f"helper reads trusted assignment storage marker `{marker}`")
            for marker in TRUST_STORAGE_MARKERS
            if marker in text
        )
        if relative != CONSUMER:
            violations.extend(
                Violation(relative, f"only the shared consumer may read `{name}`")
                for name in DISCOVERY_VARIABLES
                if name in text
            )
    return violations


def verify_orchestrated_and_standalone_skill_paths(repo: Path) -> list[Violation]:
    violations: list[Violation] = []
    for name in CANONICAL_SKILLS:
        relative = f"skills/{name}/SKILL.md"
        path = repo / relative
        if not path.is_file():
            violations.append(Violation(relative, "canonical skill entrypoint is missing"))
            continue
        text = path.read_text(encoding="utf-8")
        lines = text.splitlines()
        if len(lines) > ENTRYPOINT_MAX_LINES:
            violations.append(Violation(relative, f"entrypoint exceeds {ENTRYPOINT_MAX_LINES} lines"))
        violations.extend(
            Violation(relative, f"skill does not route through `{required}`")
            for required in SKILL_REQUIRED_TEXT
            if required not in text
        )
        violations.extend(
            Violation(relative, f"skill duplicates guide authority `{variable}`")
            for variable in DISCOVERY_VARIABLES
            if variable in text
        )
        if "ProposeRoomBrief" in text:
            violations.append(Violation(relative, "coding-agent skill claims Room Brief proposal authority"))
        for line in lines:
            if LOCK_NAMES_RE.search(line) and WRITE_RE.search(line) and LOCK_WRITER not in line:
                violations.append(Violation(relative, "review locks are written outside pr_goal_context.py"))
            if "absent" in line.lower() and "standalone" in line.lower():
                violations.append(Violation(relative, "skill maps a verifier absence to standalone"))
        if sanitize_review(text)[1]:
            violations.append(Violation(relative, "skill contains review severity labels"))
    return violations


def verify_discovery_links(repo: Path) -> list[Violation]:
    violations: list[Violation] = []
    for registry in DISCOVERY_REGISTRIES:
        for name in CANONICAL_SKILLS:
            link = repo / registry / name
            relative = f"{registry}/{name}"
            if not link.is_symlink() or link.resolve() != (repo / "skills" / name).resolve():
                violations.append(Violation(relative, "discovery link does not resolve to the canonical skill"))
        for name in ORCHESTRATOR_ONLY_SKILLS:
            if (repo / registry / name).exists() or (repo / registry / name).is_symlink():
                violations.append(Violation(f"{registry}/{name}", "orchestrator-only skill is exposed to coding agents"))
    for name in ORCHESTRATOR_ONLY_SKILLS:
        if (repo / "skills" / name).exists():
            violations.append(Violation(f"skills/{name}", "orchestrator-only skill is exposed to coding agents"))
    return violations


def validate_goal_ownership(repo: Path) -> tuple[Violation, ...]:
    violations = [
        *verify_shared_room_brief_contract(repo),
        *verify_orchestrated_and_standalone_skill_paths(repo),
        *verify_discovery_links(repo),
    ]
    return tuple(sorted(set(violations)))


def main() -> int:
    violations = validate_goal_ownership(REPO)
    for violation in violations:
        print(f"{violation.path}: {violation.message}")
    if violations:
        return 1
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
