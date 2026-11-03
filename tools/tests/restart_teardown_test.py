"""Restart teardown must signal only its live child and preserve real errors."""
import os
import select
import signal
import subprocess
import sys

import pytest

from tools.tests.restart_whole_session_test import signal_client_group, wait_for

pytestmark = pytest.mark.skipif(sys.platform == "win32", reason="Unix process groups")


@pytest.fixture
def client():
    child = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                             stdin=subprocess.PIPE, start_new_session=True)
    try:
        yield child
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)
        child.stdin.close()


@pytest.fixture
def exited_leader_group():
    # The member keeps stdout open after its leader exits. EOF proves it stopped.
    leader = subprocess.Popen([sys.executable, "-c",
        "import subprocess,sys; p=subprocess.Popen(['/bin/sleep','30']); "
        "print(p.pid,flush=True); sys.stdin.read()"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, start_new_session=True)
    killpg = os.killpg
    try:
        assert select.select([leader.stdout], [], [], 5)[0]
        int(leader.stdout.readline())
        leader.stdin.close()
        wait_for(lambda: subprocess.check_output(
            ["ps", "-p", str(leader.pid), "-o", "stat="], text=True
        ).strip().startswith("Z"), lambda: "leader did not exit", seconds=5)
        yield leader
    finally:
        if not select.select([leader.stdout], [], [], 0)[0]:
            killpg(leader.pid, signal.SIGKILL)
        assert select.select([leader.stdout], [], [], 5)[0]
        assert leader.stdout.read() == b""
        leader.wait(timeout=5)
        leader.stdin.close()
        leader.stdout.close()


@pytest.mark.parametrize("signal_name", ["SIGTERM", "SIGKILL"])
def test_exited_leader_keeps_owned_members_signalable(exited_leader_group, signal_name):
    signal_client_group(exited_leader_group, getattr(signal, signal_name))
    assert select.select([exited_leader_group.stdout], [], [], 1)[0], "owned member leaked"
    assert exited_leader_group.stdout.read() == b""


@pytest.mark.parametrize("probe_denied", [False, True])
def test_permission_error_with_exited_leader_and_live_member_is_not_hidden(
        exited_leader_group, monkeypatch, probe_denied):
    killpg = os.killpg

    def denied(pid, sig):
        if sig or probe_denied:
            raise PermissionError("live member denied")
        return killpg(pid, sig)

    monkeypatch.setattr(os, "killpg", denied)
    with pytest.raises(PermissionError, match="live member denied"):
        signal_client_group(exited_leader_group, signal.SIGTERM)
    assert not select.select([exited_leader_group.stdout], [], [], 0)[0]


@pytest.mark.parametrize("signal_name", ["SIGTERM", "SIGKILL"])
def test_live_owned_group_is_signalled_and_reaped(client, signal_name):
    sig = getattr(signal, signal_name)
    signal_client_group(client, sig)
    assert client.wait(timeout=5) == -sig


def test_reaped_client_does_not_signal_a_stale_group(client, monkeypatch):
    client.stdin.close()
    assert client.wait(timeout=5) == 0

    def stale_group(*_):
        raise AssertionError("must not signal a reaped client's process group")

    monkeypatch.setattr(os, "killpg", stale_group)
    signal_client_group(client, signal.SIGTERM)


def test_unreaped_exited_leader_without_members_can_be_cleaned(client):
    client.stdin.close()
    wait_for(lambda: subprocess.check_output(
        ["ps", "-p", str(client.pid), "-o", "stat="], text=True
    ).strip().startswith("Z"), lambda: "leader did not exit", seconds=5)
    signal_client_group(client, signal.SIGTERM)
    assert client.wait(timeout=5) == 0


def test_exit_during_signal_allows_permission_error_for_missing_group(client, monkeypatch):
    killpg = os.killpg

    def exit_before_signal(pid, sig):
        if not sig:
            return killpg(pid, sig)
        client.stdin.close()
        assert client.wait(timeout=5) == 0
        raise PermissionError("macOS exited group leader")

    monkeypatch.setattr(os, "killpg", exit_before_signal)
    signal_client_group(client, signal.SIGTERM)
    assert client.returncode == 0


def test_permission_error_for_live_client_is_not_hidden(client, monkeypatch):
    def denied(*_):
        raise PermissionError("live group denied")

    monkeypatch.setattr(os, "killpg", denied)
    with pytest.raises(PermissionError, match="live group denied"):
        signal_client_group(client, signal.SIGTERM)
    assert client.poll() is None
