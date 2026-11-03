"""Restart must relaunch the TUI, including builds with the same package version."""
import json
import os
from pathlib import Path
import select
import shutil
import signal
import struct
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


def wait_for(predicate, detail, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if value := predicate():
            return value
        time.sleep(0.05)
    raise AssertionError(detail())


def alternate_build(source, target, different_version=False):
    """Synthetic identity fixture: same code, another embedded SHA (no rebuild)."""
    identity = json.loads(subprocess.check_output([str(source), "--identity"]))
    old = identity["sha"]
    new = "b" * 40 if old != "b" * 40 else "c" * 40
    content = source.read_bytes().replace(old.encode(), new.encode())
    short = subprocess.check_output([str(source), "--version"]).decode().split("(")[1].split(")")[0]
    content = content.replace(short[:8].encode(), new[:8].encode())
    if different_version:
        version = subprocess.check_output([str(source), "--version"]).decode().split()[1]
        content = content.replace(version.encode(), ("9" + version[1:]).encode())
    target.write_bytes(content)
    target.chmod(0o755)
    if sys.platform == "darwin":
        subprocess.run(["codesign", "--force", "--sign", "-", str(target)], check=True, capture_output=True)
    assert json.loads(subprocess.check_output([str(target), "--identity"]))["sha"] == new
    return new


@pytest.mark.skipif(sys.platform == "win32", reason="Unix PTY lifecycle")
@pytest.mark.parametrize("variant,host", [
    ("different_sha", "iTerm.app"), ("different_sha", "ghostty"),
    ("same_sha", "ghostty"), ("different_version", "iTerm.app"),
    ("unhealthy", "ghostty"), ("non_dev", "ghostty"), ("registry_session", "ghostty"),
])
def test_restart_relaunches_whole_session(variant, host):
    source = os.environ.get("BUS_TEST_BINARY")
    if not source:
        pytest.skip("requires the gate's freshly built Bus binary")
    with tempfile.TemporaryDirectory(prefix="brw-", dir="/tmp") as tmp:
        run_restart(Path(tmp), Path(source).resolve(), variant, host)


def run_restart(root, source, variant, host):
    import fcntl
    import pty
    import termios

    root.chmod(0o700)
    data, home, project = [root / name for name in ("data", "home", "project")]
    for directory in (data, home, project):
        directory.mkdir(mode=0o700)
    registry_home = Path(tempfile.mkdtemp(prefix="h", dir="/tmp")) if variant == "registry_session" else None
    if registry_home:
        home = registry_home  # Keep registry socket names below macOS's 104-byte limit.
    old, new = root / "bus-A", root / "bus-B"
    shutil.copy2(source, old)
    old_sha = json.loads(subprocess.check_output([str(old), "--identity"]))["sha"]
    new_sha = old_sha
    if variant == "same_sha":
        shutil.copy2(source, new)
    else:
        new_sha = alternate_build(source, new, variant == "different_version")
    if variant == "unhealthy":
        new.write_text("#!/bin/sh\nif [ \"$1\" = --identity ]; then\n"
                       f"echo '{{\"sha\":\"{new_sha}\",\"providers\":[\"claude\"]}}'\n"
                       "else exit 42; fi\n")
    env = {"HOME": str(home), "USERPROFILE": str(home), "BUS_DATA_DIR": str(data),
           "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TERM": "xterm-256color",
           "TERM_PROGRAM": host, "SHELL": "/bin/sh"}
    if profile := os.environ.get("LLVM_PROFILE_FILE"):
        env["LLVM_PROFILE_FILE"] = profile
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    launch = [str(old), "--yolo"] if variant == "non_dev" else [str(old), "--dev", "--yolo"]
    launch_env = dict(env)
    if registry_home:
        launch_env.pop("BUS_DATA_DIR")
    client = subprocess.Popen(launch, cwd=project, env=launch_env,
                              stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)
    terminal = bytearray()
    done = threading.Event()

    def drain():
        while not done.is_set():
            try:
                if select.select([master], [], [], 0.1)[0]:
                    terminal.extend(os.read(master, 65536))
            except OSError:
                return

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    detail = lambda: terminal.decode(errors="replace")[-4000:]
    try:
        if registry_home:
            last = registry_home / ".local/share/bus/last-session"
            session = wait_for(lambda: last.read_text().strip() if last.exists() else None, detail)
            data = registry_home / ".local/share/bus/sessions" / session
            env.update(BUS_DATA_DIR=str(data), BUS_SESSION_ID=session)
        initial = wait_for(lambda: read_json(data / "restarts/build.json"), detail)
        wait_for(lambda: (data / "control.sock").exists(), detail)
        wait_for(lambda: b"Settings" in terminal, detail)
        draft = "restart-keeps-this-draft"
        os.write(master, draft.encode())
        # Differential terminal frames may split the word across cursor moves;
        # the persisted state after quit is the draft-retention oracle below.
        call = subprocess.run([str(old), "--dev", "restart", "--binary", str(new), "--confirm"],
                              cwd=project, env=env, capture_output=True, text=True, timeout=30)
        assert call.returncode == 0, call.stderr
        expected = data / "restarts/binaries" / old_sha if variant == "unhealthy" else new
        expected_sha = old_sha if variant == "unhealthy" else new_sha
        log = data / "restarts/log.jsonl"
        wait_for(lambda: log.exists() and log.read_text().strip(), detail)
        outcome = [json.loads(line) for line in log.read_text().splitlines()][-1]
        assert outcome["outcome"] == ("rolled_back" if variant == "unhealthy" else "restarted"), outcome
        # A server-only swap passes the outcome check above but leaves client A running.
        processes = subprocess.check_output(["ps", "-axo", "pid,ppid,command"], text=True)
        tui = [line for line in processes.splitlines() if str(expected) in line
               and "server" not in line and "--session-coordinator" not in line
               and "--restart-handoff" not in line and "restart --binary" not in line]
        if variant == "unhealthy":
            tui = [line for line in tui if int(line.split()[0]) != client.pid]
        assert tui, f"TUI still runs the old build; expected {expected_sha} at {expected}\n{detail()}"
        receipt = read_json(data / "restarts/tui.json")
        assert receipt["sha"] == expected_sha
        assert receipt["process"]["pid"] != client.pid
        command = subprocess.check_output(["ps", "-p", str(receipt["process"]["pid"]),
                                           "-o", "command="], text=True).strip()
        assert command.startswith(str(expected)), command
        replacement = read_json(data / "restarts/build.json")
        assert replacement["sha"] == expected_sha
        assert replacement["pid"] != initial["pid"]
        for key in ("dev", "yolo", "session_id", "explicit_data_dir"):
            assert replacement[key] == initial[key]
        assert (data / "control.sock").exists(), detail()
        assert draft in (data / "state.json").read_text(), "normal quit must persist the unsent draft"
    finally:
        done.set()
        # Only this private PTY's group and the recorded scratch server are owned.
        try:
            subprocess.run([str(old), "stop"], cwd=project, env=env,
                           capture_output=True, timeout=20, check=False)
        except subprocess.TimeoutExpired:
            pass
        try:
            os.killpg(client.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            client.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(client.pid, signal.SIGKILL)
            client.wait(timeout=5)
        for name in ("build.json", "coordinator.pid", "handoff.pid"):
            pid = read_json(data / "restarts" / name).get("pid")
            if not pid:
                continue
            command = subprocess.run(["ps", "-p", str(pid), "-o", "command="],
                                     capture_output=True, text=True).stdout.strip()
            if command.startswith((str(old), str(new))):
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        os.close(master)
        reader.join(timeout=2)
        if registry_home:
            shutil.rmtree(registry_home)
