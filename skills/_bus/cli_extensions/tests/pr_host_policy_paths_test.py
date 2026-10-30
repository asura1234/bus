"""Behavioral probes for repository policy resolution."""

import os
import subprocess
import sys
from pathlib import Path

import pytest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pr_host  # noqa: E402


POSIX_CONTROL_PATH = pytest.mark.skipif(
    os.name == "nt", reason="Windows filenames cannot contain these control characters"
)


@pytest.mark.parametrize("name", [
    pytest.param("项目", id="unicode"),
    pytest.param("main\tworktree", id="tab", marks=POSIX_CONTROL_PATH),
    pytest.param("main\nworktree", id="newline", marks=POSIX_CONTROL_PATH),
    pytest.param("main\rworktree", id="carriage-return", marks=POSIX_CONTROL_PATH),
])
def test_profile_preserves_policy_for_main_worktree_path(tmp_path, monkeypatch, name):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", os.devnull)
    repo = tmp_path / name
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    expected = {"gate_command": "just ci", "require_exact_head_gate": True}
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "just ci"\nrequire_exact_head_gate = true\n', encoding="utf-8")

    assert pr_host.repository_policy(repo) == expected
