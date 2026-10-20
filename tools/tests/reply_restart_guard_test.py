"""The restart scenario may use a worktree build, but never the live binary."""
import subprocess
import sys

import pytest

from tools.acceptance import reply_restart_test as restart


@pytest.fixture
def checkouts(tmp_path, monkeypatch):
    main = tmp_path / "main"
    worktree = tmp_path / "worktree"
    subprocess.run(["git", "init", "-q", str(main)], check=True)
    subprocess.run(["git", "-C", str(main), "-c", "user.name=Test",
                    "-c", "user.email=test@example.invalid", "commit", "-qm", "init",
                    "--allow-empty"], check=True)
    subprocess.run(["git", "-C", str(main), "worktree", "add", "--detach",
                    str(worktree)], check=True, capture_output=True)
    for checkout in (main, worktree):
        binary = checkout / "target/debug/bus"
        binary.parent.mkdir(parents=True)
        binary.write_text("stand-in; must never execute")
    monkeypatch.setattr(restart, "ROOT", worktree)
    calls = []
    monkeypatch.setattr(restart, "scenario", lambda *args: calls.append(args))
    return main, worktree, calls


def invoke(monkeypatch, binary):
    monkeypatch.setattr(sys, "argv", ["reply_restart_test.py", "--binary", str(binary)])
    restart.main()


def test_worktree_debug_binary_runs_both_restart_scenarios(checkouts, monkeypatch):
    _, worktree, calls = checkouts
    binary = worktree / "target/debug/bus"
    invoke(monkeypatch, binary)
    assert [(call[0], call[2], call[3]) for call in calls] == [
        (binary, "claude", True), (binary, "codex", False),
    ]


@pytest.mark.parametrize("alias", ["direct", "symlink", "hardlink"])
def test_live_binary_and_aliases_are_rejected_before_start(checkouts, monkeypatch, alias):
    main, worktree, calls = checkouts
    live = main / "target/debug/bus"
    candidate = live
    if alias != "direct":
        candidate = worktree / "bus-alias"
        if alias == "symlink":
            candidate.symlink_to(live)
        else:
            candidate.hardlink_to(live)
    with pytest.raises(AssertionError, match="live.*binary"):
        invoke(monkeypatch, candidate)
    assert calls == []
