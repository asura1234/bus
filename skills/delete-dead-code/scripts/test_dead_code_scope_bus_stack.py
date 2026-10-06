"""Bus stack adaptation: Rust / Python / Bun module boundaries, test paths, and assertion shapes."""

from pathlib import Path

from dead_code_scope import clone_candidates, nested_modules, rewritten_assertions, scope_files
from test_dead_code_scope import _commit, _repo, _write
from test_dead_code_scope_clones import _BODY


def test_untracked_agents_md_under_build_output_is_not_a_nested_module(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, ".gitignore", "target/\nnode_modules/\n")
    _write(repository, "vendor/libghostty-vt/AGENTS.md")
    _write(repository, "vendor/libghostty-vt/src/inspector/AGENTS.md")
    _write(repository, "vendor/libghostty-vt/node_modules/some-lib/AGENTS.md")
    _write(repository, "vendor/libghostty-vt/target/debug/build/AGENTS.md")
    _commit(repository, "base")
    # An AGENTS.md shipped by a third-party package or left in build output is not a module of this repository.
    assert nested_modules(repository, "vendor/libghostty-vt") == ("vendor/libghostty-vt/src/inspector",)


def test_rust_and_python_assertion_rewrites_are_flagged(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    integration = "tests/cli.rs"
    sibling = "src/bus/runtime_tests.rs"
    module_tests = "src/api/schema/tests.rs"
    python = "scripts/test_changelog.py"
    _write(repository, integration, "assert_eq!(one, 1);\n")
    _write(repository, sibling, "assert!(ready);\n")
    _write(repository, module_tests, "assert_ne!(one, 2);\n")
    _write(repository, python, "self.assertEqual(one, 1)\n")
    _write(repository, "src/bus/runtime.rs", "debug_assert!(ready);\n")
    _commit(repository, "base")
    _write(repository, integration, "assert_eq!(one, 2);\n")
    _write(repository, sibling, "assert!(!ready);\n")
    _write(repository, module_tests, "assert_ne!(one, 3);\n")
    _write(repository, python, "self.assertEqual(one, 2)\n")
    # Production sources are not test paths, even when they contain assertion macros.
    _write(repository, "src/bus/runtime.rs", "debug_assert!(!ready);\n")
    assert rewritten_assertions(repository) == [
        (python, 1),
        (module_tests, 1),
        (sibling, 1),
        (integration, 1),
    ]


def test_clones_cover_rust_sources_and_skip_rust_tests(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/client/one.rs", "use crate::x::Y;\n" + _BODY)
    _write(repository, "src/server/two.rs", "use std::io;\n" + _BODY)
    _write(repository, "src/server/two_tests.rs", _BODY)
    _commit(repository, "base")
    result = clone_candidates(repository, scope_files(repository, None, ["src"]))
    assert result["candidateCount"] == 1
    members = result["candidates"][0]["members"]  # type: ignore[index]
    assert [m["path"] for m in members] == ["src/client/one.rs", "src/server/two.rs"]
