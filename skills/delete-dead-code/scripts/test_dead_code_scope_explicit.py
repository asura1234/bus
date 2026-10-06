"""Explicit --directories scope and artifact structural consistency checks."""

from pathlib import Path

import pytest
from dead_code_scope import DeadCodeScopeError, handoffs, scope, tracked_paths, verify_artifact
from test_dead_code_scope import _artifact, _commit, _head, _repo, _scoped_repo, _write


# --- explicit --directories scope --------------------------------------------


def test_directories_scope_covers_every_tracked_file_not_just_changed_ones(
    tmp_path: Path,
) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _write(repository, "m/src/b.ts")
    _write(repository, "other/AGENTS.md")
    _write(repository, "other/c.ts")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "m/src/a.ts", "changed\n")
    _commit(repository, "change")
    # PR scope looks only at changed files; explicit directories cover the whole tree, which is what "sweep these trees" means.
    assert [m.name for m in scope(repository, base)] == ["m"]
    assert scope(repository, base)[0].file_count == 1
    explicit = scope(repository, directories=["m"])
    assert [m.name for m in explicit] == ["m"]
    assert explicit[0].file_count == 3  # AGENTS.md + two source files


def test_directories_scope_treats_each_directory_as_one_unit(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/own.ts")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "parent/child/deep.ts")
    _commit(repository, "base")
    # Requesting one tree yields **one** unit; nested modules are not split out. The criterion is "can it act": the canonical home is usually in
    # a sibling module, and an agent owning only one leaf can only record HANDOFF for such convergences.
    units = scope(repository, directories=["parent"])
    assert [unit.name for unit in units] == ["parent"]
    assert units[0].file_count == 4
    assert units[0].excludes == ()
    assert units[0].directories == ("parent", "parent/child")


def test_directories_scope_granularity_is_the_callers_choice(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/own.ts")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "parent/child/deep.ts")
    _commit(repository, "base")
    # Pass more subdirectories to slice finer: granularity is the caller's choice, not the script's.
    units = scope(repository, directories=["parent/child"])
    assert [unit.name for unit in units] == ["parent/child"]
    assert units[0].file_count == 2


def test_missing_directory_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _commit(repository, "base")
    # Silently scanning nothing because of a misspelled directory returns in the shape of "this is clean"; nobody can tell the scan never happened.
    with pytest.raises(DeadCodeScopeError, match="directory does not exist"):
        scope(repository, directories=["m", "typo"])


def test_directory_without_tracked_files_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _commit(repository, "base")
    (repository / "empty").mkdir()
    with pytest.raises(DeadCodeScopeError, match="has no tracked files"):
        tracked_paths(repository, ["empty"])


def test_empty_directory_list_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="must not be empty"):
        tracked_paths(tmp_path, [])


def test_scope_requires_a_range_source(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="--base or --directories"):
        scope(tmp_path)


def test_verify_uses_the_directories_scope_when_given(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "in/AGENTS.md")
    _write(repository, "in/a.ts")
    _write(repository, "out/AGENTS.md")
    _write(repository, "out/b.ts")
    _commit(repository, "base")
    artifact = _artifact(repository, "ACTED", _FOUND, "- `in/a.ts:1` — DELETED — no consumer")
    assert verify_artifact(repository, None, artifact, ["in"]) == []
    # `out/` was not requested, so it is out of scope: explicit directories are bound by the boundary check too.
    artifact = _artifact(
        repository,
        "ACTED",
        "- `out/b.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED — git grep",
        "- `out/b.ts:1` — DELETED — no consumer",
    )
    problems = verify_artifact(repository, None, artifact, ["in"])
    assert any("outside this PR's module scope" in problem for problem in problems)


# --- artifact structural checks ---------------------------------------------


_FOUND = "- `in/a.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED — git grep"
_FOUND_LIKELY = "- `in/a.ts:1` — DEAD-CODE — `foo` — desc — LIKELY — git grep"


def test_clean_with_findings_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "CLEAN", _FOUND, "- `in/a.ts:1` — KEPT — reason")
    problems = verify_artifact(repository, base, artifact)
    # This really happened in dogfooding: ten findings recorded as CLEAN made the summary report a problem module as clean.
    assert any("must be REPORTED" in p for p in problems)


def test_reported_with_findings_and_no_action_passes(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _FOUND, "- `in/a.ts:1` — HANDOFF — blocked on the out module")
    assert verify_artifact(repository, base, artifact) == []


def test_acted_without_any_action_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "ACTED", _FOUND, "- `in/a.ts:1` — KEPT — reason")
    assert any("there is no DELETED" in p for p in verify_artifact(repository, base, artifact))


def test_reported_that_actually_deleted_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _FOUND, "- `in/a.ts:1` — DELETED — gone")
    assert any("it must be ACTED" in p for p in verify_artifact(repository, base, artifact))


def test_likely_may_not_be_deleted(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "ACTED", _FOUND_LIKELY, "- `in/a.ts:1` — DELETED — gone")
    assert any("never act without confirmation" in p for p in verify_artifact(repository, base, artifact))


def test_finding_without_disposition_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _FOUND, "- None")
    assert any("no disposition" in p for p in verify_artifact(repository, base, artifact))


def test_disposition_without_finding_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "CLEAN", "- None", "- `in/a.ts:1` — KEPT — reason")
    assert any("no finding" in p for p in verify_artifact(repository, base, artifact))


def test_missing_outcome_line_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = repository / "findings.md"
    artifact.write_text(
        "# Dead Code Findings\n\n- Track: `dead`\n\n## Findings\n\n- None\n\n## Disposition\n\n- None\n", encoding="utf-8"
    )
    assert any("missing" in p and "Outcome" in p for p in verify_artifact(repository, base, artifact))


def test_missing_section_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = repository / "findings.md"
    artifact.write_text(
        "# Dead Code Findings\n\n- Track: `dead`\n- Outcome: `CLEAN`\n\n## Findings\n\n- None\n",
        encoding="utf-8",
    )
    assert any("missing `## Disposition`" in p for p in verify_artifact(repository, base, artifact))


def test_confidence_may_carry_a_trailing_qualifier(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # Observed artifacts write `— CONFIRMED (no production consumer) / test side is an observation seam —`;
    # pinning the trailing separator would flag compliant content as malformed.
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in/a.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED (production side only) / tests still read it — git grep",
        "- `in/a.ts:1` — KEPT — observation seam",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_handoffs_lists_only_handoff_anchors(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        _FOUND + "\n- `in/b.ts:2` — DEAD-CODE — `bar` — desc — CONFIRMED — git grep",
        "- `in/a.ts:1` — HANDOFF — blocked on the out module\n- `in/b.ts:2` — KEPT — seam",
    )
    assert verify_artifact(repository, base, artifact) == []
    assert handoffs(artifact) == ["in/a.ts:1"]
