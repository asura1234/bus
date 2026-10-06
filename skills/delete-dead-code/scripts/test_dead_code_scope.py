"""Contract tests for the delete-dead-code mechanical layer.

Scope derivation is the only safety boundary of this skill, so going out of bounds must be a hard failure, not a
hint: the normal, boundary, and fail-closed paths are all locked here.
"""

import subprocess
import sys
from pathlib import Path

import pytest


sys.path.insert(0, str(Path(__file__).resolve().parent))

from dead_code_scope import (  # noqa: E402
    REPOSITORY_MODULE,
    DeadCodeScopeError,
    Module,
    module_directories,
    module_of,
    nested_modules,
    plan_batches,
    scope,
    verify_artifact,
)


def _repo(tmp_path: Path) -> Path:
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "config", "user.email", "t@t"], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "config", "user.name", "t"], check=True)
    return tmp_path


def _commit(repository: Path, message: str) -> None:
    subprocess.run(["git", "-C", str(repository), "add", "-A"], check=True)
    subprocess.run(["git", "-C", str(repository), "commit", "-q", "-m", message], check=True)


def _head(repository: Path) -> str:
    return subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def _write(repository: Path, relative: str, text: str = "x\n") -> None:
    target = repository / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


# --- module_of ---------------------------------------------------------------


def test_module_is_the_nearest_agents_md_ancestor(tmp_path: Path) -> None:
    _write(tmp_path, "shell/packages/video-editor/AGENTS.md")
    _write(tmp_path, "shell/packages/video-editor/react/AGENTS.md")
    # The nearest ancestor wins; a nested module is not folded into its parent.
    assert module_of(tmp_path, "shell/packages/video-editor/react/src/a.ts") == "shell/packages/video-editor/react"
    assert module_of(tmp_path, "shell/packages/video-editor/other/b.ts") == "shell/packages/video-editor"


def test_top_level_file_maps_to_repository_module_not_dot(tmp_path: Path) -> None:
    _write(tmp_path, "AGENTS.md")
    # `"."` matches no real path prefix and reads like "the whole repository is in scope".
    assert module_of(tmp_path, ".prettierrc.json") == REPOSITORY_MODULE


def test_path_without_any_agents_md_falls_back_to_its_top_level_subtree(
    tmp_path: Path,
) -> None:
    # `cli_extensions/`, `scripts/`, and `src/` carry no `AGENTS.md`. Folding them into one "repository root" bucket,
    # touching one file would hand the agent the other two trees as well: exactly the out-of-scope shape this skill must prevent.
    assert module_of(tmp_path, "cli_extensions/lint.py") == "cli_extensions"
    assert module_of(tmp_path, "scripts/lint/deep/file.ts") == "scripts"


def test_top_level_subtree_module_gets_its_own_directory_map(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "cli_extensions/lint.py")
    _write(repository, "scripts/lint/other.ts")
    _commit(repository, "base")
    # It gets only its own subtree and does not absorb the neighboring top-level directory.
    assert module_directories(repository, "cli_extensions") == ("cli_extensions",)


# --- scope -------------------------------------------------------------------


def test_scope_counts_files_per_module_sorted_by_size(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "a/AGENTS.md")
    _write(repository, "b/AGENTS.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    _write(repository, "a/one.ts")
    _write(repository, "a/two.ts")
    _write(repository, "b/one.ts")
    _commit(repository, "change")
    assert scope(repository, base) == [
        Module(name="a", file_count=2, directories=("a",)),
        Module(name="b", file_count=1, directories=("b",)),
    ]


def test_empty_diff_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "seed.txt")
    _commit(repository, "base")
    # No change means no scope; dispatching an agent now would only send it scanning unrelated code.
    with pytest.raises(DeadCodeScopeError, match="no changed files"):
        scope(repository, "HEAD")


def test_unknown_base_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "seed.txt")
    _commit(repository, "base")
    with pytest.raises(DeadCodeScopeError, match="git diff"):
        scope(repository, "no-such-ref")


# --- plan_batches ------------------------------------------------------------


def test_batches_use_the_fewest_rounds(tmp_path: Path) -> None:
    modules = [Module(name=f"m{i}", file_count=1) for i in range(12)]
    batches = plan_batches(modules, 5)
    assert len(batches) == 3
    assert sum(len(batch) for batch in batches) == 12
    assert all(len(batch) <= 5 for batch in batches)


def test_batches_spread_the_largest_modules_across_rounds(tmp_path: Path) -> None:
    # Rounds are serial and a round takes as long as its largest module: two giant modules must land in different rounds.
    modules = [
        Module(name="huge-a", file_count=100),
        Module(name="huge-b", file_count=90),
        Module(name="small-a", file_count=2),
        Module(name="small-b", file_count=1),
    ]
    batches = plan_batches(modules, 2)
    assert len(batches) == 2
    names_per_round = [{m.name for m in batch} for batch in batches]
    assert not any({"huge-a", "huge-b"} <= names for names in names_per_round)


def test_single_module_is_one_round(tmp_path: Path) -> None:
    assert plan_batches([Module(name="only", file_count=3)], 5) == [[Module(name="only", file_count=3)]]


def test_zero_parallelism_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="must be >= 1"):
        plan_batches([Module(name="m", file_count=1)], 0)


def test_no_modules_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="no modules to plan"):
        plan_batches([], 5)


# --- verify_artifact ---------------------------------------------------------


def _scoped_repo(tmp_path: Path) -> tuple[Path, str]:
    repository = _repo(tmp_path)
    _write(repository, "in/AGENTS.md")
    _write(repository, "out/AGENTS.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    _write(repository, "in/touched.ts")
    _commit(repository, "change")
    return repository, base


def _artifact(
    repository: Path,
    outcome: str,
    findings: str,
    disposition: str,
    track: str = "dead",
) -> Path:
    path = repository / "findings.md"
    path.write_text(
        "# Dead Code Findings\n\n"
        "- Module: `in`\n"
        f"- Track: `{track}`\n"
        "- Base: `x`\n"
        f"- Outcome: `{outcome}`\n\n"
        f"## Findings\n\n{findings}\n\n"
        f"## Disposition\n\n{disposition}\n",
        encoding="utf-8",
    )
    return path


def test_finding_inside_scope_passes(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in/touched.ts:12` — DEAD-CODE — `foo` — desc — CONFIRMED — grep",
        "- `in/touched.ts:12` — KEPT — seam",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_finding_outside_scope_is_reported(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `out/untouched.ts:3` — DEAD-CODE — `bar` — desc — CONFIRMED — grep",
        "- `out/untouched.ts:3` — KEPT — seam",
    )
    offenders = [p for p in verify_artifact(repository, base, artifact) if "outside this" in p]
    assert len(offenders) == 2  # reported once for the finding line and once for the disposition line
    assert all("out/untouched.ts" in offender for offender in offenders)


def test_module_prefix_is_not_matched_by_substring(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # `in-other/` does not belong to module `in`; prefix comparison must include the separator.
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in-other/x.ts:1` — DEAD-CODE — `baz` — desc — CONFIRMED — grep",
        "- `in-other/x.ts:1` — KEPT — seam",
    )
    assert [p for p in verify_artifact(repository, base, artifact) if "outside this" in p]


def test_prose_lines_are_ignored(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # Prose mentioning an out-of-scope path is not a finding and must not trigger a scope error.
    artifact = _artifact(
        repository,
        "CLEAN",
        "- None\n\nNo out-of-scope items this round; mentioning out/untouched.ts in prose is not a finding either.",
        "- None",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_missing_artifact_fails_closed(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    with pytest.raises(DeadCodeScopeError, match="artifact does not exist"):
        verify_artifact(repository, base, repository / "absent.md")


# --- excludes (nested modules) ----------------------------------------------


def test_scope_reports_nested_modules_as_excludes(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "parent/own.ts")
    _commit(repository, "change")
    # Nested modules must be reported by the script instead of every agent re-deriving them from the tree.
    assert scope(repository, base)[0].excludes == ("parent/child",)


def test_repository_module_has_no_excludes(tmp_path: Path) -> None:
    assert nested_modules(tmp_path, REPOSITORY_MODULE) == ()


# --- directories (the module's own directory map) ------------------------------------


def test_directories_list_module_root_and_tracked_subdirectories(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _write(repository, "m/src/deep/b.ts")
    _write(repository, "m/tests/c.ts")
    _commit(repository, "base")
    assert module_directories(repository, "m") == (
        "m",
        "m/src",
        "m/src/deep",
        "m/tests",
    )


def test_directories_exclude_nested_module_subtrees(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/own/a.ts")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "parent/child/deep/b.ts")
    _commit(repository, "base")
    # A nested module is someone else's territory: it is in excludes and must not appear on my directory map either.
    assert module_directories(repository, "parent", ("parent/child",)) == (
        "parent",
        "parent/own",
    )


def test_directories_ignore_untracked_build_output(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, ".gitignore", "node_modules/\ndist/\n")
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _commit(repository, "base")
    _write(repository, "m/node_modules/pkg/index.js")
    _write(repository, "m/dist/bundle.js")
    # The directory map comes from git ls-files: build output must not eat the agent's scan budget.
    assert module_directories(repository, "m") == ("m", "m/src")


def test_repository_module_has_no_directories(tmp_path: Path) -> None:
    assert module_directories(tmp_path, REPOSITORY_MODULE) == ()


def test_scope_attaches_directories_to_each_module(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "m/src/b.ts")
    _commit(repository, "change")
    assert scope(repository, base)[0].directories == ("m", "m/src")
