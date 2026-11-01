"""Removal preserves same-name user skills that predate add-on installation."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("standalone_skill_installer", ROOT / "tools/install_skills.py")
INSTALLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALLER)


def run_addon(home, command):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home), PYTHONDONTWRITEBYTECODE="1")
    return subprocess.run([str(ROOT / "run"), command, "--yes"],
                          cwd=home, env=env, capture_output=True, text=True, timeout=60)


def tree_snapshot(path):
    return {str(file.relative_to(path)): file.read_bytes()
            for file in path.rglob("*") if file.is_file()} if path.is_dir() else None


def test_remove_preserves_identical_skills_that_predate_addon_install():
    with tempfile.TemporaryDirectory(prefix="bus-skill-user-owned-", dir="/private/tmp") as temporary:
        home = Path(temporary)
        skill = next(item for item in INSTALLER.shipped() if item["name"] == "pr")
        paths = [home / directory / "skills/pr" for _provider, directory in INSTALLER.PROVIDERS]
        for path in paths:
            path.parent.mkdir(parents=True)
            INSTALLER.copy_files(skill, path)
        before = {str(path): tree_snapshot(path) for path in paths}
        assert not INSTALLER.manifest_path(home).exists()
        installed = run_addon(home, "install-skills")
        assert installed.returncode == 0, installed.stdout + installed.stderr
        assert {str(path): tree_snapshot(path) for path in paths} == before
        manifest = json.loads(INSTALLER.manifest_path(home).read_text())
        adopted = sorted(key for key in manifest["installed"] if key.endswith("/pr"))
        assert adopted == [], adopted
        removed = run_addon(home, "remove-skills")
        assert removed.returncode == 0, removed.stdout + removed.stderr
        missing = [str(path.relative_to(home)) for path in paths if not path.is_dir()]
        assert not missing, ("remove-skills deleted pre-existing identical user skills; "
                             f"missing={missing}, adopted={adopted}; {removed.stdout}")
        assert {str(path): tree_snapshot(path) for path in paths} == before


def test_install_refuses_symlinked_config_with_existing_addon_directory():
    with tempfile.TemporaryDirectory(prefix="bus-skill-config-alias-", dir="/private/tmp") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir(mode=0o700)
        protected = root / "protected/config"
        addon = protected / "bus-skills"
        addon.mkdir(parents=True)
        (addon / "sentinel").write_text("user-owned configuration\n")
        (home / ".config").symlink_to(protected, target_is_directory=True)
        before = tree_snapshot(protected)
        installed = run_addon(home, "install-skills")
        after = tree_snapshot(protected)
        assert after == before, ("install-skills wrote through symlinked .config; "
                                 f"added={sorted(set(after) - set(before))}; "
                                 f"exit={installed.returncode}; {installed.stdout}")
        assert installed.returncode != 0, installed.stdout + installed.stderr
