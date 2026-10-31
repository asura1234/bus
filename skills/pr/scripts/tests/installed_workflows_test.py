"""Run installed scripts from another repository with a scratch HOME and offline CLI."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[4]


@pytest.fixture
def installed(tmp_path):
    assets, repo = tmp_path / "skills", tmp_path / "project"
    for name in ("pr", "merge-pr", "address-review-comments", "_bus"):
        shutil.copytree(ROOT / "skills" / name, assets / name, ignore=shutil.ignore_patterns("__pycache__"))
    home, bin_dir = tmp_path / "home", tmp_path / "bin"
    home.mkdir()
    bin_dir.mkdir()
    repo.mkdir()
    env = {k: v for k, v in os.environ.items() if k in {"PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR"}}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), GIT_CONFIG_NOSYSTEM="1",
               PATH=str(bin_dir) + os.pathsep + env["PATH"], FIXTURE_CWD_FILE=str(tmp_path / "cwd"))
    subprocess.run(["git", "init", "-qb", "feat/portable", str(repo)], env=env, check=True)
    subprocess.run(["git", "-C", str(repo), "remote", "add", "origin", "https://github.com/example/project.git"], env=env, check=True)
    return assets, repo, env


BOOTSTRAP = r"""
import json, os, pathlib, runpy, subprocess, sys
real_run = subprocess.run
view = {"number": 42, "headRefName": "feat/portable", "headRefOid": "a" * 40,
        "baseRefName": "master", "isDraft": False, "url": "https://example.test/pr/42",
        "statusCheckRollup": [], "additions": 1, "deletions": 0, "changedFiles": 1}
def offline_run(argv, *args, **kwargs):
    if argv[0] != "gh":
        return real_run(argv, *args, **kwargs)
    pathlib.Path(os.environ["FIXTURE_CWD_FILE"]).write_text(str(kwargs.get("cwd") or os.getcwd()))
    return subprocess.CompletedProcess(argv, 0, json.dumps(view if argv[1:3] == ["pr", "view"] else []), "")
subprocess.run = offline_run
sys.argv = sys.argv[1:]
sys.path.insert(0, str(pathlib.Path(sys.argv[0]).parent))
runpy.run_path(sys.argv[0], run_name="__main__")
"""


def run_script(installed, relative, *args, cwd=None):
    assets, repo, env = installed
    return subprocess.run([sys.executable, "-I", "-c", BOOTSTRAP, str(assets / relative), *args], cwd=cwd or repo,
                          env=env, text=True, capture_output=True)


def test_review_metrics_ledger_and_provider_target_cwd(installed):
    _, repo, env = installed
    done = run_script(installed, "merge-pr/scripts/review_metrics.py", "--pr", "42", "--applied", "0", "--rejected", "0")
    assert done.returncode == 0, done.stderr
    assert (repo / "temp/review-metrics.jsonl").is_file()
    assert Path(env["FIXTURE_CWD_FILE"]).read_text() == str(repo)


def test_review_lane_uses_cwd_as_default_repo(installed):
    _, repo, env = installed
    done = run_script(installed, "address-review-comments/scripts/github_review_lane.py", "fetch", "--pr", "42")
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout)["status"] == "no-comments"
    assert Path(env["FIXTURE_CWD_FILE"]).read_text() == str(repo)


def test_review_lane_relative_repo_resolves_before_chdir(installed):
    _, repo, env = installed
    done = run_script(installed, "address-review-comments/scripts/github_review_lane.py", "fetch", "--pr", "42", "--repo", "project", cwd=repo.parent)
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout)["status"] == "no-comments"
    assert Path(env["FIXTURE_CWD_FILE"]).read_text() == str(repo)


def test_merge_signals_load_host_from_bundle(installed):
    done = run_script(installed, "merge-pr/scripts/pr_signals.py", "checks", "--pr", "42")
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout)["checkCount"] == 0


def test_sanitizer_loads_bundle_dependencies(installed):
    _, repo, _ = installed
    review = repo / "review.md"
    review.write_text("P2: Handle malformed provider output.\n")
    done = run_script(installed, "address-review-comments/scripts/prepare_review_input.py",
                      "--free-form-file", str(review), "--mode", "pr", "--output", str(repo / "sanitized.md"))
    assert done.returncode == 0, done.stderr
    assert (repo / "sanitized.md").is_file()
