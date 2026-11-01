"""The run wrapper installs and removes this checkout's skills without Bus."""

import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "install_skills.py"
PROVIDERS = (".claude", ".agents", ".cursor", ".grok", ".kimi-code")


def scrubbed_env(**updates):
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("BUS_", "HERDR_", "XDG_"))
    }
    env.update(updates)
    return env


def run_tool(home, *args, stdin=""):
    return subprocess.run(
        [str(ROOT / "run"), *args],
        cwd=home,
        env=scrubbed_env(HOME=str(home), USERPROFILE=str(home)),
        input=stdin,
        capture_output=True,
        text=True,
    )


def provider_snapshot(home):
    found = {}
    for name in PROVIDERS:
        path = home / name
        if path.is_symlink() or path.exists():
            found[name] = sorted(
                str(child.relative_to(home)) for child in path.rglob("*") if child.is_file()
            )
        else:
            found[name] = None
    return found


def test_install_skills_does_not_build_bus_and_prints_each_provider(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    cargo = tmp_path / "bin" / "cargo"
    cargo.parent.mkdir()
    cargo.write_text("#!/bin/sh\nexit 9\n")
    cargo.chmod(0o755)
    env = scrubbed_env(
        HOME=str(home),
        USERPROFILE=str(home),
        PATH=f"{cargo.parent}:{os.environ['PATH']}",
    )
    result = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    lines = dict(line.split(": ", 1) for line in result.stdout.splitlines())
    for provider in ("claude", "codex", "cursor", "grok", "kimi"):
        assert lines[provider].startswith("installed "), lines
        assert "skills to " in lines[provider]
    assert (home / ".cursor/skills/pr/SKILL.md").read_bytes() == (ROOT / "skills/pr/SKILL.md").read_bytes()
    assert (home / ".grok/skills/review-pr/SKILL.md").is_file()
    assert (home / ".kimi-code/skills/pr/SKILL.md").is_file()
    assert not (home / ".local/share/bus").exists()
    assert (home / ".config/bus-skills/manifest.json").is_file()


def test_install_skills_decline_writes_nothing(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    result = run_tool(home, "install-skills", stdin="")
    assert result.returncode != 0
    assert "declined" in result.stderr
    assert provider_snapshot(home) == {name: None for name in PROVIDERS}


def test_failed_provider_exits_nonzero_and_names_the_failure(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    (home / ".claude").write_text("not a directory")
    result = run_tool(home, "install-skills", "--yes")
    assert result.returncode != 0
    lines = result.stdout.splitlines()
    assert any(line.startswith("claude: failed:") for line in lines)
    assert any(line.startswith("codex: installed ") for line in lines)
    assert (home / ".claude").read_text() == "not a directory"
    assert (home / ".agents/skills/pr/SKILL.md").is_file()


def test_install_then_remove_restores_provider_dirs_and_keeps_user_skills(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    user = home / ".cursor/skills/pr/SKILL.md"
    user.parent.mkdir(parents=True)
    user.write_text("cursor user skill\n")
    before = provider_snapshot(home)
    installed = run_tool(home, "install-skills", "--yes")
    assert installed.returncode == 0, installed.stderr
    assert user.read_text() == "cursor user skill\n"
    assert "cursor: installed " in installed.stdout
    assert (home / ".grok/skills/pr/SKILL.md").read_bytes() != b"cursor user skill\n"
    removed = run_tool(home, "remove-skills", "--yes")
    assert removed.returncode == 0, removed.stderr
    assert "claude: removed " in removed.stdout
    assert user.read_text() == "cursor user skill\n"
    after = provider_snapshot(home)
    assert after[".claude"] is None
    assert after[".agents"] is None
    assert after[".grok"] is None
    assert after[".kimi-code"] is None
    assert after[".cursor"] == before[".cursor"]


def test_remove_leaves_an_edited_owned_skill(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    assert run_tool(home, "install-skills", "--yes").returncode == 0
    edited = home / ".agents/skills/pr/SKILL.md"
    edited.write_text("my edited copy\n")
    removed = run_tool(home, "remove-skills", "--yes")
    assert removed.returncode == 0, removed.stderr
    assert edited.read_text() == "my edited copy\n"
    assert not (home / ".agents/skills/review-pr").exists()
