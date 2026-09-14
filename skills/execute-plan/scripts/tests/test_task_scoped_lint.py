"""Generation delta to Bus owner-scoped rustfmt command contract."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from task_agent_evidence import (  # noqa: E402
    EvidenceError,
    existing_touched_files,
    scoped_lint_command,
)


def _write(root: Path, relative: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("x\n", encoding="utf-8")


def test_scoped_lint_command_uses_rustfmt_without_workspace_lint() -> None:
    command = scoped_lint_command(("src/value.rs",))

    assert command == (
        "rustfmt --edition 2021 --check src/value.rs"
    )


def test_scoped_lint_command_sorts_paths() -> None:
    command = scoped_lint_command(
        ("src/b.rs", "src/a.rs"),
    )

    assert command.endswith("src/a.rs src/b.rs")


def test_scoped_lint_command_rejects_empty_paths() -> None:
    with pytest.raises(EvidenceError):
        scoped_lint_command(())


def test_existing_touched_files_drops_lint_unsupported_files(tmp_path) -> None:
    for relative in (
        "skills/AGENTS.md",
        "Cargo.toml",
        "src/value.rs",
        "src/view.rs",
        "scripts/example.py",
        "docs/commands.md",
    ):
        _write(tmp_path, relative)
    selected = existing_touched_files(
        tmp_path,
        (
            "skills/AGENTS.md",
            "Cargo.toml",
            "src/value.rs",
            "src/view.rs",
            "scripts/example.py",
            "docs/commands.md",
        ),
    )

    assert selected == (
        "src/value.rs",
        "src/view.rs",
    )


def test_existing_touched_files_drops_deleted_files(tmp_path) -> None:
    _write(tmp_path, "src/value.rs")

    selected = existing_touched_files(
        tmp_path,
        (
            "src/value.rs",
            "src/removed.rs",
        ),
    )

    assert selected == ("src/value.rs",)
