"""Unit-level worktree state: parallel invocations sharing one worktree do not interfere."""

import subprocess
from pathlib import Path

import pytest
from dead_code_scope import (
    DeadCodeScopeError,
    dirty_paths,
    preflight,
    rewritten_assertions,
    scope,
    select_units,
    verify_artifact,
)
from test_dead_code_scope import _artifact, _commit, _head, _repo, _write
from test_dead_code_scope_explicit import _FOUND


# --- unit-level worktree state: parallel invocations sharing one worktree do not interfere ---


def _shared_tree(tmp_path: Path) -> Path:
    """Two disjoint units `in` and `out` are committed, and the current branch is a feature branch."""
    repository = _repo(tmp_path)
    _write(repository, "in/README.md")
    _write(repository, "in/a.ts")
    _write(repository, "out/README.md")
    _write(repository, "out/b.ts")
    _commit(repository, "base")
    subprocess.run(["git", "-C", str(repository), "switch", "-q", "-c", "feat/x"], check=True)
    return repository


def test_preflight_ignores_dirty_files_in_another_unit(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    # Another agent is deleting dead code in `out`: that is its write set, not a reason for this invocation to stop.
    _write(repository, "out/b.ts", "changed\n")
    _write(repository, "out/new.ts")
    _write(repository, "stray-top-level.txt")
    assert preflight(repository, None, ["in"]) == []


def test_preflight_fails_on_a_dirty_file_inside_the_unit(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    _write(repository, "out/b.ts", "changed\n")
    _write(repository, "in/a.ts", "changed\n")
    _write(repository, "in/untracked.ts")
    problems = preflight(repository, None, ["in"])
    assert [problem.split(": ")[0] for problem in problems] == ["in/a.ts", "in/untracked.ts"]


def test_preflight_covers_every_unit_the_invocation_owns(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    _write(repository, "out/b.ts", "changed\n")
    # When one invocation owns two units, both are its write set.
    assert preflight(repository, None, ["in", "out"]) != []
    # When preflight takes only one of them, the other unit's changes are irrelevant.
    assert preflight(repository, None, ["in", "out"], ["in"]) == []


def test_preflight_rejects_the_main_branch(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    # `-C`: the initial branch may already be called main (`init.defaultBranch`), and `-c` would fail because it exists.
    subprocess.run(["git", "-C", str(repository), "switch", "-q", "-C", "main"], check=True)
    assert any("feature branch" in problem for problem in preflight(repository, None, ["in"]))


def test_pr_mode_preflight_includes_dirty_child_directories(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/README.md")
    _write(repository, "parent/child/README.md")
    _write(repository, "parent/child/c.ts")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "parent/p.ts")
    _commit(repository, "change")
    subprocess.run(["git", "-C", str(repository), "switch", "-q", "-c", "feat/x"], check=True)
    # PR mode owns the entire top-level subtree, including child directories.
    _write(repository, "parent/child/c.ts", "changed\n")
    assert [problem.split(": ")[0] for problem in preflight(repository, base, None)] == ["parent/child/c.ts"]
    _write(repository, "parent/p.ts", "changed\n")
    assert [problem.split(": ")[0] for problem in preflight(repository, base, None)] == [
        "parent/child/c.ts", "parent/p.ts"
    ]


def test_dirty_paths_cover_child_files_in_the_top_level_unit(
    tmp_path: Path,
) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/README.md")
    _write(repository, "parent/child/README.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "parent/p.ts")
    _write(repository, "parent/child/c.ts")
    _commit(repository, "change")
    _write(repository, "parent/child/c.ts", "changed\n")
    # Both changed paths derive one parent unit; a dirty child file must still be reported.
    assert dirty_paths(repository, scope(repository, base)) == ["parent/child/c.ts"]


def test_unknown_unit_fails_closed(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    # Silently falling back to "every unit" on a misspelled unit name would widen a unit artifact's boundary check to the whole scope.
    with pytest.raises(DeadCodeScopeError, match="--unit is not in this scope"):
        select_units(scope(repository, None, ["in", "out"]), ["inn"])


def test_verify_is_not_failed_by_another_units_in_flight_changes(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    _write(repository, "out/b.ts", "changed\n")
    _write(repository, "out/new.test.ts", "expect(x).toBe(2)\n")
    artifact = _artifact(repository, "ACTED", _FOUND, "- `in/a.ts:1` — DELETED — no consumer")
    assert verify_artifact(repository, None, artifact, ["in"]) == []
    assert verify_artifact(repository, None, artifact, ["in", "out"], ["in"]) == []


def test_verify_rejects_an_anchor_in_a_sibling_unit_of_the_same_invocation(
    tmp_path: Path,
) -> None:
    repository = _shared_tree(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        "- `out/b.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED — git grep",
        "- `out/b.ts:1` — DELETED — no consumer",
    )
    # Sibling units of the same invocation are someone else's territory too: a unit artifact stays in its own unit.
    assert verify_artifact(repository, None, artifact, ["in", "out"]) == []
    problems = verify_artifact(repository, None, artifact, ["in", "out"], ["in"])
    assert len([p for p in problems if "outside this artifact's units" in p]) == 2


def test_rewritten_assertions_only_look_at_the_owned_units(tmp_path: Path) -> None:
    repository = _shared_tree(tmp_path)
    _write(repository, "in/__tests__/a.test.ts", "expect(one).toBe(1)\n")
    _write(repository, "out/__tests__/b.test.ts", "expect(two).toBe(2)\n")
    _commit(repository, "tests")
    _write(repository, "in/__tests__/a.test.ts", "expect(one).toBe(10)\n")
    _write(repository, "out/__tests__/b.test.ts", "expect(two).toBe(20)\n")
    units = select_units(scope(repository, None, ["in", "out"]), ["in"])
    # Another unit's in-flight assertion rewrites belong to its own review, not this invocation's review list.
    assert rewritten_assertions(repository, units) == [("in/__tests__/a.test.ts", 1)]
