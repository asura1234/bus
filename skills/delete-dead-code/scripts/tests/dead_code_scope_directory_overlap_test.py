"""When explicit directories use the repository root or absolute paths, write sets must still be exclusive."""

from collections import Counter
from pathlib import Path

import pytest
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from dead_code_scope import DeadCodeScopeError, _git, plan_batches, scope, unit_pathspecs
from dead_code_scope_test import _commit, _repo, _write


@pytest.mark.parametrize("directory_form", ["repository_root", "absolute_child"])
def test_explicit_directory_forms_preserve_exclusive_write_sets(tmp_path: Path, directory_form: str) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/p.ts")
    _write(repository, "parent/child/c.ts")
    _commit(repository, "base")
    directories = (
        [".", "parent"] if directory_form == "repository_root" else ["parent", str(repository / "parent/child")]
    )

    try:
        units = scope(repository, directories=directories)
    except DeadCodeScopeError:
        return

    for batch in plan_batches(units, max_parallel=2):
        claims = Counter(
            path for unit in batch for path in _git(repository, "ls-files", "--", *unit_pathspecs(unit)).splitlines()
        )
        overlap = {path: count for path, count in claims.items() if count > 1}
        assert not overlap, overlap
