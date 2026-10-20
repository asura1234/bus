"""The live-binary guard identifies the main checkout regardless of caller environment."""
import os
import subprocess
import sys

import pytest

from tools.acceptance import reply_restart_test as restart


def make_checkouts(parent):
    main = parent / "main checkout"
    worktree = parent / "linked worktree"
    clean = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    subprocess.run(["git", "init", "-q", str(main)], check=True, env=clean)
    subprocess.run(["git", "-C", str(main), "-c", "user.name=Test",
                    "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
                    "commit", "-qm", "init", "--allow-empty", "--no-verify"],
                   check=True, env=clean)
    subprocess.run(["git", "-C", str(main), "worktree", "add", "--detach", str(worktree)],
                   check=True, capture_output=True, env=clean)
    for checkout in (main, worktree):
        binary = checkout / "target/debug/bus"
        binary.parent.mkdir(parents=True)
        binary.write_text("stand-in; must never execute")
    return main, worktree


@pytest.fixture
def scenario_calls(monkeypatch):
    calls = []
    monkeypatch.setattr(restart, "scenario", lambda *args: calls.append(args))
    return calls


def invoke(monkeypatch, binary):
    monkeypatch.setattr(sys, "argv", ["reply_restart_test.py", "--binary", str(binary)])
    restart.main()


def test_inherited_git_dir_of_another_repo_does_not_redirect_main_checkout(
        tmp_path, monkeypatch, scenario_calls):
    main, worktree = make_checkouts(tmp_path / "ours")
    other, _ = make_checkouts(tmp_path / "other")
    monkeypatch.setattr(restart, "ROOT", worktree)
    monkeypatch.setenv("GIT_DIR", str(other / ".git"))
    monkeypatch.setenv("GIT_WORK_TREE", str(other))
    with pytest.raises(AssertionError, match="live.*binary"):
        invoke(monkeypatch, main / "target/debug/bus")
    invoke(monkeypatch, other / "target/debug/bus")
    assert len(scenario_calls) == 2


def test_guard_run_from_main_checkout_allows_linked_worktree_binary(
        tmp_path, monkeypatch, scenario_calls):
    main, worktree = make_checkouts(tmp_path)
    monkeypatch.setattr(restart, "ROOT", main)
    with pytest.raises(AssertionError, match="live.*binary"):
        invoke(monkeypatch, main / "target/debug/bus")
    assert scenario_calls == []
    invoke(monkeypatch, worktree / "target/debug/bus")
    assert len(scenario_calls) == 2


def test_live_binary_reached_through_symlinked_checkout_directory_is_rejected(
        tmp_path, monkeypatch, scenario_calls):
    main, worktree = make_checkouts(tmp_path)
    alias = tmp_path / "alias-of-main"
    alias.symlink_to(main, target_is_directory=True)
    monkeypatch.setattr(restart, "ROOT", worktree)
    with pytest.raises(AssertionError, match="live.*binary"):
        invoke(monkeypatch, alias / "target/debug/bus")
    assert scenario_calls == []
