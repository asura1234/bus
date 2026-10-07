"""In a shared worktree, parent and child directories must not hold overlapping cleanup permissions."""

from pathlib import Path

from dead_code_scope import DeadCodeScopeError, scope, verify_artifact
from test_dead_code_scope import _artifact, _commit, _head, _repo, _write


def test_top_level_artifact_rejects_another_subtree(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/README.md")
    _write(repository, "other/README.md")
    _write(repository, "other/c.ts")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "parent/p.ts")
    _commit(repository, "parent change")
    artifact = _artifact(
        repository,
        "ACTED",
        "- `other/c.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED — git grep",
        "- `other/c.ts:1` — DELETED — no production consumer",
    )

    problems = verify_artifact(repository, base, artifact, unit_names=["parent"])

    assert any("outside this artifact's units" in problem for problem in problems), problems


def test_explicit_parent_and_child_never_get_overlapping_write_sets(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/README.md")
    _write(repository, "parent/p.ts")
    _write(repository, "parent/child/README.md")
    _write(repository, "parent/child/c.ts")
    _commit(repository, "base")

    try:
        units = scope(repository, directories=["parent", "parent/child"])
    except DeadCodeScopeError:
        return

    anchor = "parent/child/c.ts"
    owners = [
        unit.name
        for unit in units
        if anchor.startswith(f"{unit.name}/")
        and not any(anchor.startswith(f"{excluded}/") for excluded in unit.excludes)
    ]
    assert len(owners) == 1, owners
