"""Fail-closed language eligibility, before affected-suite narrowing."""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import PurePosixPath

from suite_references import References

PYTHON = ("scripts", "skills", "cli_extensions", "tools", "packaging")
SHARED_SKILLS = {"_bus", "gate-and-fix", "worktree-new", "rebase-origin-main"}
SHARED_FILES = {
    "run", "justfile", "Justfile", "build.rs", "build_identity.rs", "Cargo.toml", "Cargo.lock",
    "rust-toolchain", "rust-toolchain.toml", "pyproject.toml", "pytest.ini", "tox.ini",
    "setup.cfg", "setup.py", "conftest.py", ".gitmodules", ".coveragerc",
}


def kind(name: str) -> str:
    path = PurePosixPath(name)
    parts = path.parts
    if (path.name in SHARED_FILES or parts[0] in {".github", ".cargo", ".bus", ".githooks"}
            or path.name.startswith("requirements")
            or (path.suffix == ".py" and len(parts) > 1 and parts[0] == "skills" and parts[1] in SHARED_SKILLS)):
        return "shared"
    if path.suffix == ".py" and parts[0] in PYTHON:
        return "python"
    if path.suffix == ".rs" and parts[0] in {"src", "tests", "vendor"}:
        return "rust"
    if path.suffix in {".md", ".rst"} or parts[0] == "docs":
        return "docs"
    return "shared"


def rust_test(name: str) -> bool:
    path = PurePosixPath(name)
    return path.suffix == ".rs" and ("tests" in path.parts or path.name.endswith("_test.rs"))


def consumers(references: References, paths: list[str]) -> set[str]:
    seen, pending = set(), list(paths)
    while pending:
        name = pending.pop()
        for source in references.incoming(name) - seen:
            seen.add(source)
            pending.append(source)
    return seen


@dataclass(frozen=True)
class Lanes:
    python: bool
    rust: bool
    dependent_python: tuple[str, ...]
    python_reason: str
    rust_reason: str


def classify(paths: list[str], references: References) -> Lanes:
    kinds = {kind(name) for name in paths}
    # Deleted/renamed sources have lost their old graph edges. Do not infer
    # independence from absence, or from an index that could not parse a source.
    shared = "shared" in kinds or bool(paths and references.uncertain) or any(
        kind(name) != "docs" and not (references.root / name).is_file() for name in paths
    )
    incoming = consumers(references, paths)
    rust = shared or "rust" in kinds or any(name.endswith(".rs") for name in incoming)
    python = shared or "python" in kinds
    tests = {name for name in references.sources if name.endswith("_test.py") and
             PurePosixPath(name).parts[0] in PYTHON}
    dependent = tests.intersection(incoming) if rust else set()
    # Production Rust can change every binary behavior; Python CLI consumers
    # must still run. Test-only Rust does not change the shipped executable.
    if shared or any(kind(name) == "rust" and not rust_test(name) for name in paths) or (
            any(kind(name) not in {"rust", "shared"} for name in paths) and any(
                name.endswith(".rs") and not rust_test(name) for name in incoming)):
        dependent.update(name for name in tests if references.needs_binary([name]))
    return Lanes(
        python, rust, tuple(sorted(dependent)),
        "shared/unknown input" if shared else "Python changes" if python else
        "Python tests depend on changed Rust inputs" if dependent else "no Python changes",
        "shared/unknown input" if shared else "Rust changes" if "rust" in kinds else
        "Rust consumers of changed resources/scripts" if rust else "no Rust changes",
    )
