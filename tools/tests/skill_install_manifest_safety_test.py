"""Manifest integrity and exact-removal safety for the standalone add-on."""
import json
import os
import subprocess
import sys
from pathlib import Path
from unittest import mock

import pytest

from tools import install_skills

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "install_skills.py"
PROVIDERS = ("claude", "codex", "cursor", "grok", "kimi")


def run_tool(home, command):
    env = dict(os.environ, HOME=str(home), USERPROFILE=str(home))
    return subprocess.run(
        [sys.executable, str(SCRIPT), command, "--yes"],
        cwd=home,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def write_manifest(home, installed):
    path = home / ".config" / "bus-skills" / "manifest.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"schema": 1, "bundle_hash": "", "installed": installed}))


def test_remove_cannot_escape_provider_directory(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep.txt").write_text("preserve")
    write_manifest(home, {f"claude/{outside}": {"hash": install_skills.tree_hash(outside)}})

    result = run_tool(home, "remove")
    assert result.returncode == 0, result.stderr
    assert (outside / "keep.txt").read_text() == "preserve"


def test_remove_cannot_escape_with_parent_segments(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep.txt").write_text("preserve")
    write_manifest(home, {"claude/../../outside": {"hash": install_skills.tree_hash(outside)}})

    result = run_tool(home, "remove")
    assert result.returncode == 0, result.stderr
    assert (outside / "keep.txt").read_text() == "preserve"


def test_malformed_manifest_entries_fail_with_provider_summaries(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    assert run_tool(home, "install").returncode == 0
    write_manifest(home, {"claude/review-pr": "not-an-object"})

    result = run_tool(home, "remove")
    assert result.returncode != 0
    for provider in PROVIDERS:
        assert f"{provider}: failed:" in result.stdout


def test_manifest_save_failure_rolls_back_new_skill(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    skill = install_skills.shipped()[0]
    manifest = {"bundle_hash": "", "installed": {}}
    root = home / ".claude" / "skills"
    with mock.patch.object(install_skills, "save_manifest", side_effect=OSError("save failed")):
        with pytest.raises(OSError, match="save failed"):
            install_skills.install_one(home, root, "claude", skill, manifest)
    assert not (root / skill["name"]).exists()


def test_remove_preserves_preexisting_empty_provider_directory(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    (home / ".claude").mkdir()
    assert run_tool(home, "install").returncode == 0
    assert run_tool(home, "remove").returncode == 0
    assert (home / ".claude").is_dir()
