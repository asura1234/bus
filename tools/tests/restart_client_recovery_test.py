"""A delayed client must attach to the replacement that already published its PID."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

import pytest


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def wait_until(predicate, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if result := predicate():
            return result
        time.sleep(0.01)
    raise AssertionError("scratch restart timed out")


@pytest.mark.skipif(sys.platform == "win32", reason="Unix PTY and SIGSTOP regression")
@pytest.mark.parametrize("completed_handoff", [False, True])
def test_delayed_disconnect_reattaches_to_already_published_replacement(completed_handoff):
    # Unix socket paths must fit the platform's small sockaddr_un limit.
    with tempfile.TemporaryDirectory(prefix="brr-", dir="/tmp") as directory:
        run_delayed_restart(Path(directory), completed_handoff)


def run_delayed_restart(tmp_path, completed_handoff):
    import fcntl
    import pty
    import struct
    import termios

    source = os.environ.get("BUS_TEST_BINARY")
    if not source:
        pytest.skip("requires the gate's freshly built Bus binary")
    tmp_path.chmod(0o700)
    home, data, bindir, project = [tmp_path / part for part in ("home", "data", "bin", "project")]
    for directory in (home, data, bindir, project):
        directory.mkdir(mode=0o700)
    binary = bindir / "bus"
    shutil.copy2(source, binary)
    env = {"HOME": str(home), "USERPROFILE": str(home), "BUS_DATA_DIR": str(data),
           "PATH": "/usr/bin:/bin", "TERM": "xterm-256color", "SHELL": "/bin/sh"}
    identity = json.loads(subprocess.check_output([str(binary), "--identity"], env=env))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    client = subprocess.Popen([str(binary), "--dev", "--yolo"], cwd=project, env=env,
                              stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)
    terminal = bytearray()

    def drain():
        while True:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return
            if not chunk:
                return
            terminal.extend(chunk)

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    stopped = False
    helper_pid = None
    try:
        initial = wait_until(lambda: read_json(data / "restarts/build.json"))
        paths = json.loads(subprocess.check_output([str(binary), "--paths"], env=env))
        client_log = Path(paths["logs"]) / "herdr-client.log"
        wait_until(lambda: client_log.exists() and "endpoint handshake succeeded" in client_log.read_text())
        wait_until(lambda: (data / "control.sock").exists())
        os.kill(client.pid, signal.SIGSTOP)
        stopped = True
        call = subprocess.run([str(binary), "--dev", "restart", "--binary", str(binary), "--confirm"],
                              cwd=project, env=env, capture_output=True, text=True, timeout=20)
        assert call.returncode == 0, call.stderr
        helper_pid = json.loads(call.stdout)["pid"]
        replacement = wait_until(lambda: stamp if (stamp := read_json(data / "restarts/build.json")).get("pid")
                                 not in (None, initial["pid"]) else None)
        if completed_handoff:
            wait_until(lambda: not (data / "restarts/in-progress").exists())
        else:
            assert read_json(data / "restarts/in-progress"), "resume before the handoff completes"
        os.kill(client.pid, signal.SIGCONT)
        stopped = False
        # Recovery gives up after sixty seconds; survive that deadline on the same client.
        deadline = time.monotonic() + 66
        while client.poll() is None and time.monotonic() < deadline:
            time.sleep(0.1)
        assert client.poll() is None, terminal.decode(errors="replace")[-4000:]
        assert (data / "control.sock").exists(), "the original client's coordinator must survive"
        assert client_log.read_text().count("endpoint handshake succeeded") == 2
        assert replacement["sha"] == initial["sha"] == identity["sha"]
        for key in ("dev", "yolo", "session_id", "explicit_data_dir"):
            assert replacement[key] == initial[key]
    finally:
        if stopped:
            os.kill(client.pid, signal.SIGCONT)
        try:
            subprocess.run([str(binary), "stop"], cwd=project, env=env,
                           capture_output=True, timeout=20, check=False)
        except subprocess.TimeoutExpired:
            pass
        if client.poll() is None:
            client.terminate()
            try:
                client.wait(timeout=5)
            except subprocess.TimeoutExpired:
                client.kill()
                client.wait(timeout=5)
        for pid in {helper_pid, read_json(data / "restarts/build.json").get("pid"),
                    read_json(data / "restarts/coordinator.pid").get("pid")} - {None, client.pid}:
            command = subprocess.run(["ps", "-p", str(pid), "-o", "command="],
                                     capture_output=True, text=True).stdout
            if str(binary) in command:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        os.close(master)
        reader.join(timeout=2)
