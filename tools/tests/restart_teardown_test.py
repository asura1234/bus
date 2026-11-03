"""Restart teardown must signal only its live child and preserve real errors."""
import os
import signal
import subprocess
import sys

import pytest

from tools.tests.restart_whole_session_test import signal_client_group

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


def test_exit_between_poll_and_signal_allows_permission_error(client, monkeypatch):
    def exit_before_signal(*_):
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
