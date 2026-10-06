"""Duplicate-track discovery of delete-dead-code: text-clone candidates across the whole scope (CLI entry in
dead_code_scope.py)."""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass
from pathlib import Path

from dead_code_artifacts import _TEST_PATH
from dead_code_units import REPOSITORY_MODULE, DeadCodeScopeError, _git, scope, tracked_paths


DEFAULT_CLONE_MIN_LINES = 8
"""Minimum effective lines of a clone window (after dropping blank, comment, import, and bracket-only lines).
Repeats shorter than this are mostly boilerplate."""

DEFAULT_CLONE_LIMIT = 200
CLONE_BOILERPLATE_OCCURRENCES = 10
"""A window occurring more often than this is boilerplate (generated code, protocol tables) and is not paired;
otherwise the pair count explodes quadratically. The number of skipped windows is written to the output, never
silently truncated."""

SOURCE_SUFFIXES = frozenset(
    {
        ".ts",
        ".tsx",
        ".js",
        ".jsx",
        ".mjs",
        ".cjs",
        ".py",
        ".c",
        ".cc",
        ".cpp",
        ".h",
        ".hpp",
        ".m",
        ".mm",
        ".rs",
        ".kt",
        ".kts",
        ".java",
        ".swift",
    }
)


@dataclass(frozen=True)
class _NormalizedLine:
    number: int
    text: str


_TRIVIAL_LINE = re.compile(r"^[\s{}()\[\];,]*$")
_COMMENT_LINE = re.compile(r"^(?://|/\*|\*|\*/|#(?:\s|$|!))")
_IMPORT_LINE = re.compile(
    r"^(?:import\b|export\s+\*|export\s+(?:type\s+)?\{[^}]*\}\s+from\b|from\s+\S+\s+import\b"
    r"|#\s*(?:include|import|pragma)\b|using\s+namespace\b|use\s+[\w:]+"
    r"|package\s+[\w.]+;?$|@testable\s+import\b)"
)
_GENERATED_PATH = re.compile(r"(?:^|/)(?:generated|generated-conformance|__generated__)/")
_FIXTURE_PATH = re.compile(r"(?:^|/)(?:__fixtures__|fixtures)/")


def _normalized_lines(text: str) -> list[_NormalizedLine]:
    """Effective lines after dropping lines that carry no logic: blank, comment, import/include, bracket-only.

    Clones compare logic, not layout: without dropping them, two identical functions fall out of window alignment
    because of an import block or one comment line, while two unrelated files get flagged as clones because of the
    same stack of `});`.
    """
    lines: list[_NormalizedLine] = []
    for number, raw in enumerate(text.splitlines(), start=1):
        stripped = " ".join(raw.split())
        if (
            not stripped
            or _TRIVIAL_LINE.match(stripped)
            or _COMMENT_LINE.match(stripped)
            or _IMPORT_LINE.match(stripped)
        ):
            continue
        lines.append(_NormalizedLine(number, stripped))
    return lines


def scope_files(repository: Path, base: str | None = None, directories: list[str] | None = None) -> list[str]:
    """Every tracked file in scope: the duplicate track looks for candidates across the whole scope, not per module."""
    if directories:
        return tracked_paths(repository, directories)
    files: set[str] = set()
    for module in scope(repository, base, None):
        if module.name == REPOSITORY_MODULE:
            files.update(
                entry for entry in _git(repository, "ls-files", "-z").split("\0") if entry and "/" not in entry
            )
            continue
        excluded = tuple(f"{item}/" for item in module.excludes)
        files.update(
            entry
            for entry in _git(repository, "ls-files", "-z", "--", module.name).split("\0")
            if entry and not entry.startswith(excluded)
        )
    return sorted(files)


def clone_candidates(
    repository: Path,
    paths: list[str],
    min_lines: int = DEFAULT_CLONE_MIN_LINES,
    include_tests: bool = False,
) -> dict[str, object]:
    """Text-clone candidates across files (and at non-overlapping positions in one file), by effective lines
    descending.

    This is **discovery**, not a verdict: it only answers "these two spans are line-for-line identical once layout
    is removed". Whether the duplication is intentional, where it converges, and whether it can converge at all is
    decided by guide.md "Duplicate track". It cannot find semantic duplicates (same responsibility, different
    code); the responsibility-inventory comparison covers that half.
    """
    if min_lines < 2:
        raise DeadCodeScopeError(f"--min-lines must be >= 2, got {min_lines}")
    corpus: dict[str, list[_NormalizedLine]] = {}
    for path in paths:
        if Path(path).suffix not in SOURCE_SUFFIXES or _GENERATED_PATH.search(path):
            continue
        if not include_tests and (_TEST_PATH.search(path) or _FIXTURE_PATH.search(path)):
            continue
        target = repository / path
        if not target.is_file():
            continue
        try:
            text = target.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        lines = _normalized_lines(text)
        if len(lines) >= min_lines:
            corpus[path] = lines

    windows: dict[str, list[tuple[str, int]]] = {}
    for path, lines in corpus.items():
        for index in range(len(lines) - min_lines + 1):
            digest = hashlib.sha1(
                "\n".join(line.text for line in lines[index : index + min_lines]).encode()
            ).hexdigest()
            windows.setdefault(digest, []).append((path, index))

    skipped = 0
    pairs: dict[tuple[str, str], list[tuple[int, int]]] = {}
    for occurrences in windows.values():
        if len(occurrences) < 2:
            continue
        if len(occurrences) > CLONE_BOILERPLATE_OCCURRENCES:
            skipped += 1
            continue
        for left_index, left in enumerate(occurrences):
            for right in occurrences[left_index + 1 :]:
                first, second = sorted((left, right))
                if first[0] == second[0] and second[1] - first[1] < min_lines:
                    continue
                pairs.setdefault((first[0], second[0]), []).append((first[1], second[1]))

    candidates: list[dict[str, object]] = []
    for (left_path, right_path), matches in pairs.items():
        runs: list[list[tuple[int, int]]] = []
        for match in sorted(set(matches)):
            if runs and match == (runs[-1][-1][0] + 1, runs[-1][-1][1] + 1):
                runs[-1].append(match)
            else:
                runs.append([match])
        for run in runs:
            length = len(run) + min_lines - 1
            members = []
            for path, start in ((left_path, run[0][0]), (right_path, run[0][1])):
                lines = corpus[path]
                members.append(
                    {
                        "path": path,
                        "startLine": lines[start].number,
                        "endLine": lines[start + length - 1].number,
                    }
                )
            candidates.append({"lines": length, "members": members})

    candidates.sort(
        key=lambda item: (
            -int(item["lines"]),  # type: ignore[call-overload]
            [(m["path"], m["startLine"]) for m in item["members"]],  # type: ignore[union-attr]
        )
    )
    return {
        "minLines": min_lines,
        "filesScanned": len(corpus),
        "skippedBoilerplateWindows": skipped,
        "candidateCount": len(candidates),
        "candidates": candidates,
    }
