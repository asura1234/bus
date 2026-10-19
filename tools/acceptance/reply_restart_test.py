"""Actual server/coordinator process restart with a durable, not-yet-consumed Stop.

Called by the dev-tools Rust integration target with its exact built binary.
No installed providers, desktop commands, user HOME or live Bus are reachable.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.acceptance.harness_test import Bus, CliError  # noqa: E402


def until(predicate, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("condition did not arrive before deadline")


def stop_spooled(data):
    for path in data.glob("callbacks/*/*.json"):
        try:
            record = json.loads(path.read_text())
            if isinstance(record, dict) and record.get("value", {}).get("hook_event_name") == "Stop":
                return True
        except (ValueError, OSError):
            continue
    return False


def scenario(binary, root, provider, master):
    root.mkdir()
    work = root / "work"
    work.mkdir()
    home = root / "home"
    home.mkdir()
    fake_bin = root / "bin"
    fake_bin.mkdir()
    for name in ["claude", "codex", "open", "osascript", "terminal-notifier", "afplay"]:
        (fake_bin / name).symlink_to(binary)
    data = root / "data"
    # Closed environment: provider and effects names resolve only to our stand-ins.
    env = {"PATH": f"{fake_bin}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home),
           "BUS_DATA_DIR": str(data), "BUS_TUI_FAKE_AGENT": "1", "TERM": "xterm-256color",
           "SHELL": "/bin/sh", "XDG_CONFIG_HOME": str(root / "xdg"),
           "BUS_TUI_EFFECTS": str(root / "effects.jsonl")}
    bus = Bus(binary, root, env, workspace=work)
    began = time.monotonic()
    try:
        bus.start()
        room = "master" if master else "bus"
        args = ["agent", "add", "--room", room, "--name", "restart-agent", "--provider", provider,
                "--pwd", str(work), "--consent-hooks"]
        if master:
            args += ["--orchestrates", "bus"]
        bus.cli(*args)
        until(lambda: any(a["status"] == "idle" for a in bus.cli("state")["agents"]))
        sent = bus.cli("send", "--room", room, "--to", "restart-agent", "--text",
                       "fake:work 3 fake:raw-reply restart-proof")
        message = str(sent["message_id"])
        until(lambda: bus.cli("message", "status", message)["requests"][0]["start_bound"])
        # Stop only this scratch coordinator while its native provider keeps running.
        os.kill(bus.child.pid, signal.SIGSTOP)
        until(lambda: stop_spooled(data), timeout=15)
        stopped = subprocess.run([str(binary), "stop"], env=env, capture_output=True, text=True,
                                 timeout=20, check=False)
        assert stopped.returncode == 0, stopped.stderr
        os.kill(bus.child.pid, signal.SIGKILL)
        bus.child.wait(timeout=10)
        os.close(bus.master)
        bus.master = None
        bus.pump_thread.join(timeout=5)
        # The same data directory now belongs to a newly started server + coordinator.
        bus.start()

        def delivered():
            try:
                status = bus.cli("message", "status", message)
                return status if status["complete"] else None
            except CliError:
                return None

        status = until(delivered)
        assert status["requests"][0]["stage"] == "replied", status
        assert status["requests"][0]["reply"]["text"] == "restart-proof", status
        history = bus.cli("history", "--room", room)
        rows = [row for row in history["messages"] if str(row["prompt"]["id"]) == message]
        assert len(rows) == 1, history
        assert len(rows[0]["delivery"]["requests"]) == 1, history
        assert rows[0]["delivery"]["requests"][0]["reply"]["text"] == "restart-proof", history
        print(json.dumps({"provider": provider, "room": room, "message": message,
                          "seconds": round(time.monotonic() - began, 3), "result": "PASS"}))
    finally:
        if bus.child is not None and bus.child.poll() is None:
            os.kill(bus.child.pid, signal.SIGCONT)
        bus.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    assert not binary.is_relative_to(ROOT / "target/debug"), "protected target/debug is forbidden"
    with tempfile.TemporaryDirectory(prefix="br-restart-", dir="/private/tmp") as directory:
        root = Path(directory)
        scenario(binary, root / "claude", "claude", True)
        scenario(binary, root / "codex", "codex", False)


if __name__ == "__main__":
    main()
