"""Existing provider registry directories are outside add-on ownership."""
import json
import os
from pathlib import Path
import subprocess
import sys

from tools import install_skills

SCRIPT = Path(install_skills.__file__).resolve()


def run_tool(home, verb):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home), PYTHONDONTWRITEBYTECODE="1")
    return subprocess.run([sys.executable, str(SCRIPT), verb, "--yes"],
                          cwd=home, env=env, capture_output=True, text=True, timeout=60)


def test_remove_preserves_preexisting_empty_provider_skills_directories(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    roots = [home / directory / "skills" for _provider, directory in install_skills.PROVIDERS]
    for root in roots:
        root.mkdir(parents=True)
    installed = run_tool(home, "install")
    assert installed.returncode == 0, installed.stdout + installed.stderr
    removed = run_tool(home, "remove")
    assert removed.returncode == 0, removed.stdout + removed.stderr
    missing = [str(root.relative_to(home)) for root in roots if not root.is_dir()]
    assert not missing, f"remove deleted pre-existing empty provider skills directories: {missing}"


def test_remove_prunes_new_skills_roots_but_preserves_existing_provider_homes(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    for _provider, directory in install_skills.PROVIDERS:
        (home / directory).mkdir()
    installed = run_tool(home, "install")
    assert installed.returncode == 0, installed.stdout + installed.stderr
    manifest = json.loads(install_skills.manifest_path(home).read_text())
    assert set(manifest["created_dirs"]) == {
        f"{directory}/skills" for _provider, directory in install_skills.PROVIDERS
    }
    removed = run_tool(home, "remove")
    assert removed.returncode == 0, removed.stdout + removed.stderr
    for _provider, directory in install_skills.PROVIDERS:
        assert (home / directory).is_dir()
        assert not (home / directory / "skills").exists()
    assert json.loads(install_skills.manifest_path(home).read_text())["created_dirs"] == []


def test_remove_prunes_new_provider_homes_and_roots_after_reinstall(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    for _ in range(2):
        installed = run_tool(home, "install")
        assert installed.returncode == 0, installed.stdout + installed.stderr
    removed = run_tool(home, "remove")
    assert removed.returncode == 0, removed.stdout + removed.stderr
    assert not any((home / directory).exists() for _provider, directory in install_skills.PROVIDERS)
    manifest = json.loads(install_skills.manifest_path(home).read_text())
    assert manifest["created_dirs"] == []
    assert manifest["installed"] == {}


def test_remove_preserves_roots_without_explicit_ownership_in_legacy_manifest(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    installed = run_tool(home, "install")
    assert installed.returncode == 0, installed.stdout + installed.stderr
    path = install_skills.manifest_path(home)
    manifest = json.loads(path.read_text())
    manifest["created_dirs"] = [directory for _provider, directory in install_skills.PROVIDERS]
    path.write_text(json.dumps(manifest))
    removed = run_tool(home, "remove")
    assert removed.returncode == 0, removed.stdout + removed.stderr
    for _provider, directory in install_skills.PROVIDERS:
        root = home / directory / "skills"
        assert root.is_dir()
        assert list(root.iterdir()) == []
