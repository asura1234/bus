"""Duplicate track: text-clone discovery across the whole scope."""

from pathlib import Path

import pytest
from dead_code_scope import DeadCodeScopeError, clone_candidates, scope_files
from test_dead_code_scope import _commit, _head, _repo, _write


# --- clones: text-clone discovery across the whole scope -----------------------------


_BODY = "".join(f"  total = total + value{index} * {index}\n" for index in range(10))


def _clone_repo(tmp_path: Path) -> Path:
    repository = _repo(tmp_path)
    _write(repository, "a/README.md")
    _write(repository, "b/README.md")
    _write(repository, "a/one.ts", "import { x } from './x'\n\nfunction one() {\n" + _BODY + "}\n")
    # Different layout (extra comment, blank line, indentation) is still the same logic.
    _write(
        repository,
        "b/two.ts",
        "// copied\nfunction two() {\n\n" + _BODY.replace("  ", "    ") + "}\n",
    )
    _write(repository, "b/other.ts", "".join(f"let v{i} = {i}\n" for i in range(20)))
    _commit(repository, "base")
    return repository


def test_clones_find_cross_module_copies_ignoring_layout(tmp_path: Path) -> None:
    repository = _clone_repo(tmp_path)
    result = clone_candidates(repository, scope_files(repository, None, ["a", "b"]), min_lines=8)
    assert result["candidateCount"] == 1
    candidate = result["candidates"][0]  # type: ignore[index]
    assert candidate["lines"] == 10
    assert [m["path"] for m in candidate["members"]] == ["a/one.ts", "b/two.ts"]
    # Line numbers point back to the original file: imports, blank lines, and comments are skipped, but the range uses real line numbers.
    assert candidate["members"][0]["startLine"] == 4
    assert candidate["members"][1]["startLine"] == 4


def test_clones_below_min_lines_are_not_reported(tmp_path: Path) -> None:
    repository = _clone_repo(tmp_path)
    result = clone_candidates(repository, scope_files(repository, None, ["a", "b"]), min_lines=11)
    assert result["candidateCount"] == 0


def test_clones_skip_tests_unless_asked(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "a/src/one.ts", _BODY)
    _write(repository, "a/src/__tests__/one.test.ts", _BODY)
    _commit(repository, "base")
    files = scope_files(repository, None, ["a"])
    assert clone_candidates(repository, files)["candidateCount"] == 0
    assert clone_candidates(repository, files, include_tests=True)["candidateCount"] == 1


def test_clones_ignore_trivial_and_import_lines(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    noise = "".join(f"import {{ m{i} }} from './m{i}'\n}}\n);\n" for i in range(12))
    _write(repository, "a/x.ts", noise)
    _write(repository, "a/y.ts", noise)
    _commit(repository, "base")
    assert clone_candidates(repository, scope_files(repository, None, ["a"]))["candidateCount"] == 0


def test_clones_do_not_pair_overlapping_windows_in_one_file(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "a/x.ts", "".join("  step()\n" for _ in range(12)))
    _commit(repository, "base")
    # The same line repeated 12 times: the windows overlap each other and are not two independent copies.
    assert clone_candidates(repository, scope_files(repository, None, ["a"]))["candidateCount"] == 0


def test_clone_min_lines_must_be_meaningful(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="--min-lines"):
        clone_candidates(tmp_path, [], min_lines=1)


def test_scope_files_in_pr_mode_cover_whole_touched_modules(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "a/README.md")
    _write(repository, "a/old.ts")
    _write(repository, "a/nested/README.md")
    _write(repository, "a/nested/n.ts")
    _write(repository, "z/README.md")
    _write(repository, "z/z.ts")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "a/new.ts")
    _commit(repository, "change")
    # Duplicates must be compared with **unchanged** files in the module, so the corpus is the whole touched module, not just changed files;
    # nested directories are included; untouched top-level subtrees are not.
    assert scope_files(repository, base) == [
        "a/README.md", "a/nested/README.md", "a/nested/n.ts", "a/new.ts", "a/old.ts"
    ]
