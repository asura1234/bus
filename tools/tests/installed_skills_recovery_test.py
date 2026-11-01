"""Installed copies stay manageable after manifest loss and keep their links inside the install."""
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "install_skills.py"


def _bus(home, *args, check=True, binary=None):
    del binary
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home))
    result = subprocess.run(
        [sys.executable, str(TOOL), *args], env=env, cwd=home, capture_output=True, text=True, check=False
    )
    if check and result.returncode != 0:
        raise AssertionError(result.stdout + result.stderr)
    return result


def test_reinstall_after_a_lost_manifest_adopts_byte_identical_bus_copies(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    _bus(home, "install", "--yes")
    # A missing manifest is adopted again when the bytes still match; a corrupt one blocks install.
    (home / ".config/bus-skills/manifest.json").unlink()
    assert _bus(home, "install", "--yes").returncode == 0
    report = json.loads(_bus(home, "status").stdout)
    states = {skill["state"] for skill in report["skills"]}
    assert states == {"installed"}, report
    manifest = json.loads((home / ".config/bus-skills/manifest.json").read_text())
    assert len(manifest["installed"]) == len(report["skills"])


def test_a_symlinked_provider_home_is_not_reported_as_existing_user_skills(tmp_path):
    home = tmp_path / "home"
    (home / "dotfiles/claude").mkdir(parents=True)
    (home / ".claude").symlink_to(home / "dotfiles/claude")
    result = _bus(home, "install", "--yes")
    assert result.returncode == 0, result.stderr
    claude = [skill for skill in json.loads(_bus(home, "status").stdout)["skills"] if skill["provider"] == "claude"]
    reported_as_user_skill = [skill["name"] for skill in claude
                              if skill["state"] == "conflict" and not Path(skill["path"]).exists()]
    assert reported_as_user_skill == [], result.stderr.splitlines()[:1]


def test_installed_markdown_links_resolve_inside_the_installed_bundle(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    _bus(home, "install", "--yes")
    root = home / ".claude/skills"
    broken = []
    for skill in (path.name for path in root.iterdir() if path.is_dir() and path.name not in {"_bus", "pr", "merge-pr", "address-review-comments"}):
        for page in (root / skill).rglob("*.md"):
            for target in re.findall(r"\]\(([^)#\s]+)(?:#[^)]*)?\)", page.read_text()):
                if "://" not in target and not (page.parent / target).exists():
                    broken.append(f"{page.relative_to(root)} -> {target}")
    assert broken == []


@pytest.mark.parametrize("metadata", [".DS_Store", "Thumbs.db"])
def test_disposable_os_metadata_keeps_bus_ownership(tmp_path, metadata):
    home = tmp_path / "home"
    home.mkdir()
    _bus(home, "install", "--yes")
    (home / ".claude/skills/review-pr" / metadata).write_bytes(b"metadata")
    report = json.loads(_bus(home, "status").stdout)
    review = next(s for s in report["skills"] if s["provider"] == "claude" and s["name"] == "review-pr")
    assert review["state"] == "installed"


def test_install_leaves_existing_settings_unchanged(tmp_path):
    home = tmp_path / "home"
    data = home / ".local/share/bus"
    data.mkdir(parents=True)
    settings = {"feishu_recipient": "ou_mine", "sidebar_width": 40, "disk_warning_gb": 5, "disk_critical_gb": 20}
    (data / "settings.json").write_text(json.dumps(settings))
    before = (data / "settings.json").read_bytes()
    result = _bus(home, "install", "--yes", check=False)
    assert result.returncode == 0, result.stderr
    assert (data / "settings.json").read_bytes() == before
    assert (home / ".agents/skills/review-pr/SKILL.md").is_file()


@pytest.mark.parametrize("name", [".env", ".config/review.toml", "notes.pyc"])
def test_upgrades_preserve_user_authored_files(tmp_path, name):
    home = tmp_path / "home"
    home.mkdir()
    _bus(home, "install", "--yes")
    path = home / ".agents/skills/review-pr" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    content = b"USER_REVIEW_ENDPOINT=http://localhost:43210\n"
    path.write_bytes(content)
    before = json.loads(_bus(home, "status").stdout)
    assert _bus(home, "install", "--yes").returncode == 0
    after = json.loads(_bus(home, "status").stdout)
    assert path.is_file(), f"upgrade deleted user file {name}"
    assert path.read_bytes() == content
    for report in (before, after):
        state = next(item["state"] for item in report["skills"]
                     if item["provider"] == "codex" and item["name"] == "review-pr")
        assert state == "conflict", "authored files are modifications, not disposable metadata"
