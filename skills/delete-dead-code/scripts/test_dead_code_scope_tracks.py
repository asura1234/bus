"""Mechanical enumeration for the main-agent REVIEW, and mutual exclusion of the two tracks' actions."""

from pathlib import Path

from dead_code_scope import consolidations, rewritten_assertions, verify_artifact
from test_dead_code_scope import _artifact, _commit, _repo, _scoped_repo, _write
from test_dead_code_scope_explicit import _FOUND


# --- mechanical enumeration for the main-agent REVIEW --------------------------------


def test_consolidations_lists_only_consolidated_anchors(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        "- `in/a.ts:1` — DUPLICATE[G1] — `foo` — d — CONFIRMED — g\n"
        "- `in/b.ts:2` — DUPLICATE[G1] — `foo` — d — CONFIRMED — g",
        "- `in/a.ts:1` — CANONICAL — production wiring\n- `in/b.ts:2` — CONSOLIDATED — moved to the canonical implementation",
        track="duplicate",
    )
    # Only consolidations need item-by-item main-agent review: pure deletion never changes behavior, a reversed consolidation does.
    assert [anchor for _, anchor, _ in consolidations([artifact])] == ["in/b.ts:2"]


def test_rewritten_assertions_flags_changed_but_not_deleted_expectations(
    tmp_path: Path,
) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/__tests__/a.test.ts", "expect(one).toBe(1)\n")
    _write(repository, "src/prod.ts", "export const one = 1\n")
    _commit(repository, "base")
    _write(repository, "src/__tests__/a.test.ts", "expect(one).toBe(2)\n")
    # A rewritten assertion is the strongest signal of a reversed consolidation: the survivor behaves differently, and the only way to green is changing the expectation.
    # The review target is this invocation's uncommitted rewrites.
    assert rewritten_assertions(repository) == [("src/__tests__/a.test.ts", 1)]


def test_deleting_a_zombie_test_is_not_a_rewritten_assertion(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/__tests__/a.test.ts", "expect(one).toBe(1)\n")
    _commit(repository, "base")
    (repository / "src/__tests__/a.test.ts").unlink()
    # Deleting a zombie test wholesale is a normal disposition and must not flood the main agent's review list.
    assert rewritten_assertions(repository) == []


def test_production_source_changes_are_not_counted_as_assertions(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/prod.ts", "const x = 1\n")
    _commit(repository, "base")
    _write(repository, "src/prod.ts", "expect(x).toBe(1)\n")
    assert rewritten_assertions(repository) == []


# --- two tracks: dead code only deletes, duplicates only consolidate ------------------


def _dup(anchor: str, group: str = "G1", confidence: str = "CONFIRMED") -> str:
    return f"- `{anchor}` — DUPLICATE[{group}] — `foo` — d — {confidence} — g"


def test_missing_track_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = repository / "findings.md"
    artifact.write_text(
        "# Dead Code Findings\n\n- Outcome: `CLEAN`\n\n## Findings\n\n- None\n\n## Disposition\n\n- None\n",
        encoding="utf-8",
    )
    assert any("Track" in p for p in verify_artifact(repository, base, artifact))


def test_dead_track_may_not_consolidate(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # A consolidation mixed into the deletion track gets waved through as a deletion during review: exactly how a reversed consolidation slips past every defense.
    artifact = _artifact(repository, "ACTED", _FOUND, "- `in/a.ts:1` — CONSOLIDATED — merged")
    assert any("does not belong to the dead track" in p for p in verify_artifact(repository, base, artifact))


def test_dead_track_rejects_duplicate_findings(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _dup("in/a.ts:1"), "- `in/a.ts:1` — KEPT — r")
    assert any("DUPLICATE does not belong to the dead track" in p for p in verify_artifact(repository, base, artifact))


def test_duplicate_track_may_not_delete(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        f"{_dup('in/a.ts:1')}\n{_dup('in/a.ts:9')}",
        "- `in/a.ts:1` — DELETED — gone\n- `in/a.ts:9` — KEPT — r",
        track="duplicate",
    )
    assert any("DELETED does not belong to the duplicate track" in p for p in verify_artifact(repository, base, artifact))


def test_consolidated_group_with_one_canonical_passes(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        f"{_dup('in/a.ts:1')}\n{_dup('in/a.ts:9')}",
        "- `in/a.ts:1` — CANONICAL — production wiring\n- `in/a.ts:9` — CONSOLIDATED — moved to a.ts:1",
        track="duplicate",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_consolidated_group_needs_exactly_one_canonical(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        f"{_dup('in/a.ts:1')}\n{_dup('in/a.ts:9')}",
        "- `in/a.ts:1` — CONSOLIDATED — m\n- `in/a.ts:9` — CONSOLIDATED — m",
        track="duplicate",
    )
    assert any("exactly one" in p for p in verify_artifact(repository, base, artifact))


def test_canonical_without_consolidation_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        f"{_dup('in/a.ts:1')}\n{_dup('in/a.ts:9')}",
        "- `in/a.ts:1` — CANONICAL — r\n- `in/a.ts:9` — KEPT — drifted, product decision",
        track="duplicate",
    )
    assert any("has no CONSOLIDATED" in p for p in verify_artifact(repository, base, artifact))


def test_group_with_a_likely_member_may_not_be_consolidated(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        f"{_dup('in/a.ts:1')}\n{_dup('in/a.ts:9', confidence='LIKELY')}",
        "- `in/a.ts:1` — CANONICAL — r\n- `in/a.ts:9` — CONSOLIDATED — m",
        track="duplicate",
    )
    assert any("LIKELY" in p for p in verify_artifact(repository, base, artifact))


def test_single_member_group_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _dup("in/a.ts:1"), "- `in/a.ts:1` — KEPT — r", track="duplicate")
    assert any("at least 2 are required" in p for p in verify_artifact(repository, base, artifact))


def test_duplicate_without_group_id_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in/a.ts:1` — DUPLICATE — `foo` — d — CONFIRMED — g",
        "- `in/a.ts:1` — KEPT — r",
        track="duplicate",
    )
    assert any("no group id" in p for p in verify_artifact(repository, base, artifact))
