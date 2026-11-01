"""The run wrapper installs this checkout's skills and reports what changed."""

import json
import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def checkout_identity():
    repo = subprocess.check_output(
        ["bash", "-c", 'cd -- "$(dirname -- "$1")" && pwd', "_", str(ROOT / "run")],
        text=True,
    ).strip()
    sha = subprocess.check_output(
        ["git", "-C", repo, "rev-parse", "HEAD"],
        text=True,
        env={
            key: value
            for key, value in os.environ.items()
            if key not in {"GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"}
        },
    ).strip()
    root = subprocess.check_output(
        ["python3", "-I", "-c", "import os,sys; print(os.path.realpath(sys.argv[1]))", repo],
        text=True,
    ).strip()
    return repo, sha, root


def write_bus(path, sha, root, log=None):
    record = f'printf "%s\\n" "$@" >> "{log}"\n' if log else ""
    path.write_text(
        "#!/bin/sh\n"
        'if [ "$1" = skills ] && [ "$2" = status ]; then\n'
        f"  printf '%s\\n' '{{\"build_sha\":\"{sha}\",\"source_root\":\"{root}\"}}'\n"
        "  exit 0\n"
        "fi\n"
        f"{record}"
        'printf "%s\\n" "$@"\n'
    )
    path.chmod(0o755)


def scrubbed_env(**updates):
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("BUS_", "HERDR_", "XDG_"))
    }
    env.update(updates)
    return env


def test_install_skills_builds_and_passes_consent_in_scratch_home(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    _, sha, root = checkout_identity()
    bus = bin_dir / "bus"
    write_bus(bus, sha, root)
    cargo = bin_dir / "cargo"
    cargo.write_text(
        '#!/bin/sh\nprintf \'{"reason":"compiler-artifact","executable":"%s"}\\n\' "$FAKE_BUS"\n'
    )
    cargo.chmod(0o755)
    env = scrubbed_env(
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
    assert "not this checkout" not in result.stderr
    assert result.stdout.splitlines() == ["skills", "install", "--yes"]


def test_install_skills_does_not_run_stale_binary_if_build_fails(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    cargo = tmp_path / "cargo"
    cargo.write_text("#!/bin/sh\nexit 9\n")
    cargo.chmod(0o755)
    env = scrubbed_env(
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


def test_install_skills_rejects_a_shared_target_binary_from_another_checkout(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    repo, sha, root = checkout_identity()
    private = f"{repo}/target/install-skills"
    foreign = tmp_path / "foreign-bus"
    local = tmp_path / "local-bus"
    foreign_log = tmp_path / "foreign.log"
    local_log = tmp_path / "local.log"
    write_bus(foreign, "0123456789abcdef0123456789abcdef01234567", "/other/checkout", foreign_log)
    write_bus(local, sha, root, local_log)
    calls = tmp_path / "cargo-calls"
    cargo = tmp_path / "bin" / "cargo"
    cargo.parent.mkdir()
    cargo.write_text(
        "#!/bin/sh\n"
        f'printf "%s\\n" "$CARGO_TARGET_DIR" >> "{calls}"\n'
        f'if [ "$CARGO_TARGET_DIR" = "{private}" ]; then\n'
        f'  printf \'{{"reason":"compiler-artifact","executable":"%s"}}\\n\' "{local}"\n'
        "else\n"
        f'  printf \'{{"reason":"compiler-artifact","executable":"%s"}}\\n\' "{foreign}"\n'
        "fi\n"
    )
    cargo.chmod(0o755)
    env = scrubbed_env(
        HOME=str(home),
        USERPROFILE=str(home),
        PATH=f"{cargo.parent}:{os.environ['PATH']}",
        CARGO_TARGET_DIR=str(tmp_path / "shared-target"),
    )
    result = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == ["skills", "install", "--yes"]
    assert calls.read_text().splitlines() == [str(tmp_path / "shared-target"), private]
    foreign_args = foreign_log.read_text().split() if foreign_log.exists() else []
    assert "install" not in foreign_args
    assert local_log.read_text().splitlines() == ["skills", "install", "--yes"]


def test_install_skills_refuses_when_the_rebuilt_binary_is_still_foreign(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    foreign = tmp_path / "foreign-bus"
    foreign_log = tmp_path / "foreign.log"
    write_bus(foreign, "0123456789abcdef0123456789abcdef01234567", "/other/checkout", foreign_log)
    cargo = tmp_path / "bin" / "cargo"
    cargo.parent.mkdir()
    cargo.write_text(
        "#!/bin/sh\n"
        f'printf \'{{"reason":"compiler-artifact","executable":"%s"}}\\n\' "{foreign}"\n'
    )
    cargo.chmod(0o755)
    env = scrubbed_env(
        HOME=str(home),
        USERPROFILE=str(home),
        PATH=f"{cargo.parent}:{os.environ['PATH']}",
        CARGO_TARGET_DIR=str(tmp_path / "shared-target"),
    )
    result = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "still does not match" in result.stderr
    foreign_args = foreign_log.read_text().split() if foreign_log.exists() else []
    assert "install" not in foreign_args
    assert not (home / ".agents").exists()


def test_install_skills_installs_checkout_bundle_and_preserves_user_skill(tmp_path):
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
    env = scrubbed_env(
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
    actions = {
        (entry["provider"], entry["name"]): entry["action"] for entry in report["skills"]
    }
    assert states[("claude", "pr")] == "conflict"
    assert actions[("claude", "pr")] == "preserved-conflict"
    assert states[("codex", "pr")] == "installed"
    assert actions[("codex", "pr")] == "installed"
    for provider in ("cursor", "grok", "kimi"):
        assert states[(provider, "pr")] == "installed"
        assert actions[(provider, "pr")] == "installed"
    again = subprocess.run(
        [str(ROOT / "run"), "install-skills", "--yes"],
        env=env,
        capture_output=True,
        text=True,
    )
    assert again.returncode == 0, again.stderr
    second = {
        (entry["provider"], entry["name"]): entry["action"]
        for entry in json.loads(again.stdout)["skills"]
    }
    assert second[("codex", "pr")] == "unchanged"
    assert second[("claude", "pr")] == "preserved-conflict"
    for provider, directory in (
        ("cursor", ".cursor"),
        ("grok", ".grok"),
        ("kimi", ".kimi-code"),
    ):
        assert second[(provider, "pr")] == "unchanged"
        assert (home / directory / "skills/pr/SKILL.md").read_bytes() == (
            ROOT / "skills/pr/SKILL.md"
        ).read_bytes()
    assert custom.read_text() == "my custom skill\n"
    assert (home / ".agents/skills/pr/SKILL.md").read_bytes() == (
        ROOT / "skills/pr/SKILL.md"
    ).read_bytes()
    assert any(str(custom.parent) in warning for warning in report["warnings"])
