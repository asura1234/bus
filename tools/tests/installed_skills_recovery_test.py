"""Installed copies stay manageable after manifest loss and keep their links inside the install."""
import json
import os
import re
import subprocess
from pathlib import Path

import pytest


def _bus(home, *args, check=True):
    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's exact built Bus binary")
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home))
    return subprocess.run([binary, "skills", *args], env=env, cwd=home, capture_output=True, text=True, check=check)


def test_reinstall_after_a_lost_manifest_adopts_byte_identical_bus_copies(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    _bus(home, "install", "--yes")
    # A corrupt manifest blocks both status and install; removing it is the only way back.
    (home / ".local/share/bus/skills-manifest.json").unlink()
    report = json.loads(_bus(home, "install", "--yes").stdout)
    states = {skill["state"] for skill in report["skills"]}
    assert states == {"installed"}, report["warnings"][:2]
    manifest = json.loads((home / ".local/share/bus/skills-manifest.json").read_text())
    assert len(manifest["installed"]) == len(report["skills"])


def test_a_symlinked_provider_home_is_not_reported_as_existing_user_skills(tmp_path):
    home = tmp_path / "home"
    (home / "dotfiles/claude").mkdir(parents=True)
    (home / ".claude").symlink_to(home / "dotfiles/claude")
    result = _bus(home, "install", "--yes")
    claude = [skill for skill in json.loads(result.stdout)["skills"] if skill["provider"] == "claude"]
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


def test_finder_metadata_in_an_installed_copy_keeps_bus_ownership(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    _bus(home, "install", "--yes")
    (home / ".claude/skills/review-pr/.DS_Store").write_bytes(b"finder")
    report = json.loads(_bus(home, "status").stdout)
    review = next(s for s in report["skills"] if s["provider"] == "claude" and s["name"] == "review-pr")
    assert review["state"] == "installed"


def test_recording_skills_consent_preserves_existing_bus_settings(tmp_path):
    home = tmp_path / "home"
    data = home / ".local/share/bus"
    data.mkdir(parents=True)
    settings = {"feishu_recipient": "ou_mine", "sidebar_width": 40, "disk_warning_gb": 5, "disk_critical_gb": 20}
    (data / "settings.json").write_text(json.dumps(settings))
    before = (data / "settings.json").read_bytes()
    result = _bus(home, "install", "--yes", check=False)
    assert result.returncode != 0
    assert str(data / "settings.json") in result.stderr
    assert (data / "settings.json").read_bytes() == before
    saved = json.loads((data / "settings.json").read_text())
    assert (saved.get("feishu_recipient"), saved.get("sidebar_width")) == ("ou_mine", 40)
