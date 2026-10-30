"""The run wrapper delegates installation from its checkout and forwards consent."""

import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def test_install_skills_builds_and_passes_consent_in_scratch_home(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    bus = bin_dir / "bus"
    bus.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
    bus.chmod(0o755)
    cargo = bin_dir / "cargo"
    cargo.write_text(
        '#!/bin/sh\nprintf \'{"reason":"compiler-artifact","executable":"%s"}\\n\' "$FAKE_BUS"\n'
    )
    cargo.chmod(0o755)
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith(("BUS_", "HERDR_", "XDG_"))
    }
    env.update(
        HOME=str(home),
        USERPROFILE=str(home),
        PATH=f"{bin_dir}:{os.environ['PATH']}",
        FAKE_BUS=str(bus),
    )
    result = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == ["skills", "install", "--yes"]


def test_install_skills_does_not_run_stale_binary_if_build_fails(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    cargo = tmp_path / "cargo"
    cargo.write_text("#!/bin/sh\nexit 9\n")
    cargo.chmod(0o755)
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith(("BUS_", "HERDR_", "XDG_"))
    }
    env.update(
        HOME=str(home), USERPROFILE=str(home), PATH=f"{tmp_path}:{os.environ['PATH']}"
    )
    result = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 9
    assert not list(home.iterdir())


def test_install_skills_installs_checkout_bundle_and_preserves_user_skill(tmp_path):
    import json
    import pytest

    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's exact built Bus binary")
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    custom = home / ".claude/skills/pr/SKILL.md"
    custom.parent.mkdir(parents=True)
    custom.write_text("my custom skill\n")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    cargo = bin_dir / "cargo"
    cargo.write_text(
        '#!/bin/sh\nprintf \'{"reason":"compiler-artifact","executable":"%s"}\\n\' "$TEST_BINARY"\n'
    )
    cargo.chmod(0o755)
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith(("BUS_", "HERDR_", "XDG_"))
    }
    env.update(
        HOME=str(home),
        USERPROFILE=str(home),
        PATH=f"{bin_dir}:{os.environ['PATH']}",
        TEST_BINARY=binary,
    )
    declined = subprocess.run(
        [str(ROOT / "run"), "install-skills"],
        env=env,
        input="",
        capture_output=True,
        text=True,
    )
    assert declined.returncode != 0
    assert not (home / ".agents").exists()
    result = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    states = {
        (entry["provider"], entry["name"]): entry["state"] for entry in report["skills"]
    }
    assert states[("claude", "pr")] == "conflict"
    assert states[("codex", "pr")] == "installed"
    assert custom.read_text() == "my custom skill\n"
    assert (home / ".agents/skills/pr/SKILL.md").read_bytes() == (
        ROOT / "skills/pr/SKILL.md"
    ).read_bytes()
    assert any(str(custom.parent) in warning for warning in report["warnings"])
