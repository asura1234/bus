"""The restart guard's own tests must not write into a repository named by inherited GIT_* env."""
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def git(repo, *args):
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    return subprocess.run(["git", "-C", str(repo), *args], env=env, check=True,
                          capture_output=True, text=True).stdout


def test_guard_tests_leave_a_repository_named_by_inherited_git_dir_untouched(tmp_path):
    foreign = tmp_path / "foreign"
    git(tmp_path, "init", "-q", str(foreign))
    git(foreign, "-c", "user.name=T", "-c", "user.email=t@example.invalid",
        "commit", "-qm", "seed", "--allow-empty")
    before = (git(foreign, "rev-list", "--all"), git(foreign, "worktree", "list", "--porcelain"))
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_DIR=str(foreign / ".git"), GIT_WORK_TREE=str(foreign))
    run = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider",
         "tools/tests/reply_restart_guard_test.py"],
        cwd=ROOT, env=env, capture_output=True, text=True)
    after = (git(foreign, "rev-list", "--all"), git(foreign, "worktree", "list", "--porcelain"))
    assert after == before, f"foreign repo mutated\n{run.stdout[-2000:]}"
    assert run.returncode == 0, run.stdout[-2000:]
