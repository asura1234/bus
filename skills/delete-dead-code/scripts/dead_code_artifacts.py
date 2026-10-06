"""Artifact layer of delete-dead-code: findings verification and the review enumeration of consolidations and
rewritten assertions (CLI entry in dead_code_scope.py)."""

from __future__ import annotations

import re
from pathlib import Path

from dead_code_units import DeadCodeScopeError, Module, _git, _in_scope, scope, select_units, unit_pathspecs


OUTCOMES = ("CLEAN", "ACTED", "REPORTED")
"""The three terminal states of an artifact. `REPORTED` means "found real dead code but could not act". Without
it, cross-module findings could only be squeezed into `CLEAN`, and the summary would report a problem module as
clean. This really happened in dogfooding."""

TRACKS = ("dead", "duplicate")
"""The track an artifact belongs to. The two tracks' actions are mutually exclusive: the dead-code track only
deletes, the duplicate track only consolidates. Mixing both actions in one artifact is exactly the shape of the
incident where "a dead-code deletion changed production behavior": the consolidation hid among deletions and was
waved through as a deletion during review."""

TRACK_KINDS = {"dead": ("DEAD-CODE", "DEAD-BRANCH"), "duplicate": ("DUPLICATE",)}
TRACK_DISPOSITIONS = {
    "dead": ("DELETED", "KEPT", "HANDOFF"),
    "duplicate": ("CONSOLIDATED", "CANONICAL", "KEPT", "HANDOFF"),
}
TRACK_ACTIONS = {"dead": "DELETED", "duplicate": "CONSOLIDATED"}

# Confidence only needs to be "one field starting with `— `"; it need not be followed directly by a separator.
# Artifacts are handwritten and agents often append a qualifier after the field (observed:
# `— CONFIRMED (no production consumer) / test side is an observation seam —`). Pinning the trailing separator
# would only flag compliant content as malformed; what must be locked is which confidence was declared.
_FINDING = re.compile(
    r"^\s*[-*]\s+`(?P<path>[^`:]+):(?P<line>\d+)`\s+—\s+"
    r"(?P<kind>DUPLICATE|DEAD-BRANCH|DEAD-CODE)(?:\[(?P<group>[A-Za-z0-9_-]+)\])?\s+—.*?"
    r"—\s*(?P<confidence>CONFIRMED|LIKELY)\b"
)
_DISPOSITION = re.compile(
    r"^\s*[-*]\s+`(?P<path>[^`:]+):(?P<line>\d+)`\s+—\s+"
    r"(?P<disposition>DELETED|CONSOLIDATED|CANONICAL|KEPT|HANDOFF)\b"
)
_ANY_ANCHOR = re.compile(r"^\s*[-*]\s+`(?P<path>[^`:]+):(?P<line>\d+)`")
_OUTCOME = re.compile(r"^\s*[-*]\s+Outcome:\s*`(?P<outcome>[A-Z]+)`\s*$")
_TRACK = re.compile(r"^\s*[-*]\s+Track:\s*`(?P<track>[a-z]+)`\s*$")
_HEADING = re.compile(r"^##\s+(?P<title>.+?)\s*$")


# Rust `assert!` / `assert_eq!` / `assert_ne!` (and `debug_` variants), Python `assertEqual`, and Bun `expect` are
# all assertions.
_ASSERTION = re.compile(
    r"\b(?:expect|assert(?:[A-Z]\w*)?|(?:debug_)?assert(?:_eq|_ne)?!|EXPECT_[A-Z_]+|ASSERT_[A-Z_]+)\s*\("
)
_TEST_PATH = re.compile(
    r"(?:^|/)(?:__tests__|tests?)/"
    r"|(?:\.test\.[jt]sx?|_test\.(?:cc|cpp)|(?:^|/)test_[^/]+\.py|(?:^|/|_)tests?\.rs)$"
)


def _sections(text: str) -> dict[str, list[str]]:
    """Split by `## ` headings. Artifacts are handwritten by agents; missing sections are more common than
    reordered ones, and both must be detectable."""
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        heading = _HEADING.match(line)
        if heading:
            current = heading.group("title")
            sections.setdefault(current, [])
            continue
        if current is not None:
            sections[current].append(line)
    return sections


def verify_artifact(
    repository: Path,
    base: str | None,
    artifact: Path,
    directories: list[str] | None = None,
    unit_names: list[str] | None = None,
) -> list[str]:
    """Return every problem in the artifact; an empty list means it passes.

    Two kinds of checks: **scope** (finding paths must fall inside the artifact's own units) and
    **self-consistency** (Outcome, Findings, and Disposition must not contradict each other). Scope alone is not
    enough: `CLEAN` with 20 findings makes the summary lie just as well, and that really happened in dogfooding.

    `unit_names` are the artifact's own units (without them, every unit in scope): sibling units of the same
    invocation are someone else's territory too. Verification reads only the artifact and the scope derivation,
    **never worktree state**: other units' in-flight changes are unrelated to this artifact and must not fail it.
    """
    if not artifact.is_file():
        raise DeadCodeScopeError(f"artifact does not exist: {artifact}")
    text = artifact.read_text(encoding="utf-8")
    modules = select_units(scope(repository, base, directories), unit_names)
    where = (
        f"is outside this artifact's units ({', '.join(sorted(module.name for module in modules))})"
        if unit_names
        else "is outside this PR's module scope"
    )
    problems: list[str] = []

    outcome: str | None = None
    track: str | None = None
    for line in text.splitlines():
        if outcome is None and (match := _OUTCOME.match(line)):
            outcome = match.group("outcome")
        if track is None and (match := _TRACK.match(line)):
            track = match.group("track")
    if outcome is None:
        problems.append(f"{artifact}: missing `- Outcome: ...` line")
    elif outcome not in OUTCOMES:
        problems.append(f"{artifact}: unknown Outcome {outcome}; legal values {'/'.join(OUTCOMES)}")
    if track is None:
        problems.append(f"{artifact}: missing `- Track: ...` line (dead or duplicate)")
    elif track not in TRACKS:
        problems.append(f"{artifact}: unknown Track {track}; legal values {'/'.join(TRACKS)}")
        track = None

    sections = _sections(text)
    for required in ("Findings", "Disposition"):
        if required not in sections:
            problems.append(f"{artifact}: missing `## {required}` section")

    for number, line in enumerate(text.splitlines(), start=1):
        anchor = _ANY_ANCHOR.match(line)
        if anchor and not _in_scope(anchor.group("path"), modules):
            problems.append(f"{artifact}:{number}: {anchor.group('path')} {where}")

    findings: dict[str, str] = {}
    groups: dict[str, str] = {}
    for line in sections.get("Findings", []):
        match = _FINDING.match(line)
        if match:
            anchor = f"{match.group('path')}:{match.group('line')}"
            findings[anchor] = match.group("confidence")
            kind = match.group("kind")
            if track is not None and kind not in TRACK_KINDS[track]:
                problems.append(
                    f"{artifact}: {anchor} kind {kind} does not belong to the {track} track "
                    f"(legal values {'/'.join(TRACK_KINDS[track])})"
                )
            if kind == "DUPLICATE":
                if match.group("group") is None:
                    problems.append(f"{artifact}: {anchor} is DUPLICATE but has no group id `DUPLICATE[<group>]`")
                else:
                    groups[anchor] = match.group("group")
        elif _ANY_ANCHOR.match(line):
            problems.append(f"{artifact}: malformed finding line: {line.strip()[:80]}")

    dispositions: dict[str, str] = {}
    for line in sections.get("Disposition", []):
        match = _DISPOSITION.match(line)
        if match:
            dispositions[f"{match.group('path')}:{match.group('line')}"] = match.group("disposition")
        elif _ANY_ANCHOR.match(line):
            problems.append(f"{artifact}: malformed disposition line: {line.strip()[:80]}")

    for anchor in sorted(set(findings) - set(dispositions)):
        problems.append(f"{artifact}: {anchor} has a finding but no disposition")
    for anchor in sorted(set(dispositions) - set(findings)):
        problems.append(f"{artifact}: {anchor} has a disposition but no finding")

    if track is not None:
        for anchor, disposition in sorted(dispositions.items()):
            if disposition not in TRACK_DISPOSITIONS[track]:
                problems.append(
                    f"{artifact}: {anchor} disposition {disposition} does not belong to the {track} track "
                    f"(legal values {'/'.join(TRACK_DISPOSITIONS[track])})"
                )

    for anchor, disposition in sorted(dispositions.items()):
        if disposition in ("DELETED", "CONSOLIDATED") and findings.get(anchor) == "LIKELY":
            problems.append(f"{artifact}: {anchor} is LIKELY but {disposition}; never act without confirmation")

    if track == "duplicate":
        problems.extend(_group_problems(artifact, groups, findings, dispositions))

    action = TRACK_ACTIONS.get(track or "", "DELETED/CONSOLIDATED")
    acted = any(value in ("DELETED", "CONSOLIDATED") for value in dispositions.values())
    if outcome == "CLEAN" and findings:
        problems.append(
            f"{artifact}: Outcome is CLEAN but there are {len(findings)} findings; "
            "found-but-could-not-act must be REPORTED"
        )
    if outcome == "ACTED" and not acted:
        problems.append(f"{artifact}: Outcome is ACTED but there is no {action}")
    if outcome == "REPORTED" and acted:
        problems.append(f"{artifact}: Outcome is REPORTED but there is a {action}; it must be ACTED")
    if outcome == "REPORTED" and not findings:
        problems.append(f"{artifact}: Outcome is REPORTED but there are no findings")
    return problems


def _group_problems(
    artifact: Path,
    groups: dict[str, str],
    findings: dict[str, str],
    dispositions: dict[str, str],
) -> list[str]:
    """Duplicate-group self-consistency: a consolidation has exactly one survivor, and every member is confirmed.

    "Consolidating" moves N copies onto one: without `CANONICAL` nobody can say which one stays, and two of them
    means nothing was consolidated. If any member of the group is only `LIKELY` (not sure it is equivalent to the
    others), the whole group must stay untouched.
    """
    problems: list[str] = []
    members: dict[str, list[str]] = {}
    for anchor, group in groups.items():
        members.setdefault(group, []).append(anchor)
    for group, anchors in sorted(members.items()):
        if len(anchors) < 2:
            problems.append(f"{artifact}: duplicate group {group} has {len(anchors)} member(s); at least 2 are required")
        values = [dispositions.get(anchor) for anchor in anchors]
        canonical = values.count("CANONICAL")
        consolidated = values.count("CONSOLIDATED")
        if consolidated and canonical != 1:
            problems.append(
                f"{artifact}: duplicate group {group} has CONSOLIDATED but {canonical} CANONICAL; exactly one is required"
            )
        if canonical and not consolidated:
            problems.append(f"{artifact}: duplicate group {group} marks CANONICAL but has no CONSOLIDATED")
        if consolidated and any(findings.get(anchor) == "LIKELY" for anchor in anchors):
            problems.append(
                f"{artifact}: duplicate group {group} has a LIKELY member; a group not confirmed equivalent "
                "must not be consolidated"
            )
    return problems


def consolidations(artifacts: list[Path]) -> list[tuple[Path, str, str]]:
    """Every Disposition entry marked CONSOLIDATED across the artifacts: `(artifact, anchor, rationale)`.

    These are what the main agent **must** review one by one. Consolidation is the only action of this skill that
    can cause a product incident: a reversed direction changes production behavior, and it is always green,
    because the tests get pointed at the wrong copy along with it.
    """
    found: list[tuple[Path, str, str]] = []
    for artifact in artifacts:
        for line in _sections(artifact.read_text(encoding="utf-8")).get("Disposition", []):
            match = _DISPOSITION.match(line)
            if match and match.group("disposition") == "CONSOLIDATED":
                anchor = f"{match.group('path')}:{match.group('line')}"
                found.append((artifact, anchor, line.strip()))
    return found


def rewritten_assertions(repository: Path, units: list[Module] | None = None) -> list[tuple[str, int]]:
    """Assertion lines **rewritten rather than deleted** in this invocation's uncommitted changes, only inside
    `units` (without them, the whole tree).

    A flipped assertion is the strongest signal of a reversed consolidation: when the surviving copy behaves
    differently from what production ran, the only way to turn the tests green is to change the expectation.
    Pure deletion (removing a zombie test wholesale) does not count; that is a normal disposition.

    Both scope modes diff against `HEAD`, the same basis as the A5 delta: expectation changes the PR itself already
    committed, and rewrites the dead-code track already LANDed, are not this track's artifact to explain; the diff
    is taken per unit, so other units' in-flight rewrites are not either.
    """
    pathspec_sets = [unit_pathspecs(unit) for unit in units] if units is not None else [["."]]
    diff = "\n".join(_git(repository, "diff", "-U0", "HEAD", "--", *pathspecs) for pathspecs in pathspec_sets)
    current: str | None = None
    counts: dict[str, int] = {}
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            path = line[6:]
            current = path if _TEST_PATH.search(path) else None
            continue
        if current and line.startswith("+") and not line.startswith("+++"):
            if _ASSERTION.search(line):
                counts[current] = counts.get(current, 0) + 1
    return sorted(counts.items())


def handoffs(artifact: Path) -> list[str]:
    """Anchors marked HANDOFF: the deletion crosses into another module and the main agent must pair it."""
    text = artifact.read_text(encoding="utf-8")
    return [
        f"{match.group('path')}:{match.group('line')}"
        for line in _sections(text).get("Disposition", [])
        if (match := _DISPOSITION.match(line)) and match.group("disposition") == "HANDOFF"
    ]
