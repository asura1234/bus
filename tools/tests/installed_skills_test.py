"""Installed workflow resources must resolve independently of Bus's checkout."""
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def test_review_scripts_find_bundled_support_and_target_the_callers_repo(tmp_path):
    skills = tmp_path / "user" / ".agents" / "skills"
    home = skills.parents[1]
    home.mkdir(parents=True)
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home))
    installed = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "install_skills.py"), "install", "--yes"],
        env=env, cwd=tmp_path, capture_output=True, text=True, check=False,
    )
    assert installed.returncode == 0, installed.stdout + installed.stderr
    repo = tmp_path / "other-repo"
    repo.mkdir()
    for args in (["init", "--initial-branch=feature"], ["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--allow-empty", "-m", "base"]):
        subprocess.run(["git", *args], cwd=repo, env=env, check=True, capture_output=True)
    (repo / "example.txt").write_text("changed")
    subprocess.run(["git", "add", "example.txt"], cwd=repo, env=env, check=True)
    subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-m", "change"], cwd=repo, env=env, check=True, capture_output=True)
    (repo / ".git/info/exclude").write_text("temp/\n")
    lock = repo / "temp/review-pr/feature"
    lock.mkdir(parents=True)
    (lock / ".locked-goal").write_text("Test installed skills\n")
    (lock / ".locked-non-goals").write_text("None\n")
    result = subprocess.run([sys.executable, str(skills / "review-pr/scripts/review_round.py"), "--base", "HEAD~1", "--reviewer", "installed"], cwd=repo, env=env, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "ROUND=1" in result.stdout
    assert (lock / "installed").is_dir()
    assert not (skills.parent / "temp").exists()


def test_binary_launch_never_offers_or_installs_skills(tmp_path):
    import select
    import time
    import pytest

    if sys.platform == "win32":
        pytest.skip("PTY launch contract is covered on Unix")
    import pty
    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's exact built Bus binary")
    home = tmp_path / "home"
    home.mkdir()
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home), BUS_DATA_DIR="relative-refused-before-server")

    master, slave = pty.openpty()
    process = subprocess.Popen([binary], cwd=home, env=env, stdin=slave, stdout=slave, stderr=slave)
    os.close(slave)
    output = b""
    deadline = time.monotonic() + 15
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                output += data
            elif process.poll() is not None:
                break
        process.wait(timeout=2)
        assert process.returncode != 0, "relative data root must prevent server startup"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
    text = output.decode(errors="replace")
    assert "Install?" not in text
    assert "Install the following" not in text
    assert not (home / ".agents").exists()
    assert not (home / ".claude").exists()
    assert not (home / ".local/share/bus/settings.json").exists()
    fresh = tmp_path / "piped"
    fresh.mkdir()
    env["HOME"] = str(fresh)
    env["USERPROFILE"] = str(fresh)
    result = subprocess.run([binary], env=env, cwd=fresh, input="yes\n", capture_output=True, text=True)
    assert "Install the following" not in result.stderr
    assert "Install?" not in result.stderr
    assert not (fresh / ".agents").exists()
    assert not (fresh / ".local/share/bus/settings.json").exists()


def test_bus_refuses_a_test_launch_with_protected_home_before_any_state_write(tmp_path):
    import pytest
    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's exact built Bus binary")
    protected = tmp_path / "protected-home"
    protected.mkdir()
    scratch = tmp_path / "scratch-home"
    scratch.mkdir()
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env["TEST_BUS_PROTECTED_HOME"] = str(protected)
    # The relative root keeps the pre-guard binary from starting anything in RED.
    for args, home, data in [([], protected, "relative-refused-root"), (["resume", "--last"], scratch, str(protected / ".local/share/bus"))]:
        env.update(HOME=str(home), USERPROFILE=str(home), BUS_DATA_DIR=data)
        result = subprocess.run([binary, *args], env=env, cwd=tmp_path, input="", capture_output=True, text=True)
        assert result.returncode != 0
        assert "test isolation refused" in result.stderr, result.stdout + result.stderr
        assert not list(protected.iterdir()), "the guard must run before any state write"


def test_corrupt_manifest_stops_install_without_copying_or_writing_settings(tmp_path):
    home = tmp_path / "home"
    state = home / ".config/bus-skills"
    state.mkdir(parents=True)
    (state / "manifest.json").write_text("corrupt")
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home))
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "install_skills.py"), "install", "--yes"],
        env=env, cwd=tmp_path, capture_output=True, text=True,
    )
    assert result.returncode != 0
    assert "manifest.json" in result.stdout + result.stderr
    assert not (home / ".local/share/bus/settings.json").exists()
    assert not (home / ".agents").exists()


def test_sitrep_is_installed_for_claude_and_codex(tmp_path):
    import pytest

    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's exact built Bus binary")
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home))
    subprocess.run([binary, "skills", "install", "--yes"], env=env, cwd=tmp_path, capture_output=True, check=True)
    for provider in (".claude", ".agents"):
        skill = home / provider / "skills/sitrep"
        assert (skill / "SKILL.md").is_file()
        assert (skill / "scripts/render.py").is_file()
        assert (skill / "scripts/marker.py").is_file()
        assert (skill / "references/sitrep-format.md").is_file()
