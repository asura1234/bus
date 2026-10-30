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
           "SHELL": "/bin/bash", "XDG_CONFIG_HOME": str(root / "xdg"),
           "BUS_TUI_EFFECTS": str(root / "effects.jsonl")}
    bus = Bus(binary, root, env, workspace=work)
    began = time.monotonic()
    try:
        bus.start()
        # Keep the default first work room as the observer across restart;
        # neither reply source can be marked seen by the new TUI's startup view.
        bus.cli("room", "create", "reply-work")
        room = "master" if master else "reply-work"
        args = ["agent", "add", "--room", room, "--name", "restart-agent", "--provider", provider,
                "--pwd", str(work), "--consent-hooks"]
        if master:
            args += ["--orchestrates", "reply-work"]
        bus.cli(*args)
        until(lambda: any(a["status"] == "idle" for a in bus.cli("state")["agents"]))
        bus.cli("room", "focus", "bus")

        def durable():
            return json.loads((data / "state.json").read_text())["state"]

        observer = next(r["id"] for r in durable()["rooms"].values() if r["name"] == "bus")
        source = next(r["id"] for r in durable()["rooms"].values() if r["name"].lower() == room)
        until(lambda: durable()["visible_room"] == observer)
        before = {key: value["unread_count"] for key, value in durable()["rooms"].items()}
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
        # A history map cannot reveal duplicate publication. Unviewed room
        # counters and standalone notices are independent visible side effects.
        state = durable()
        assert state["visible_room"] != source, state["visible_room"]
        expected = dict(before)
        expected[str(source)] += 1
        assert {key: value["unread_count"] for key, value in state["rooms"].items()} == expected, state
        for target in ["master", "reply-work"]:
            other = bus.cli("history", "--room", target)
            assert not [row for row in other["messages"]
                        if row["prompt"]["text"] == "restart-proof" and row["delivery"] is None], other
        for _ in range(3):
            bus.cli("message", "status", message)
            bus.cli("history", "--room", room)
        assert {key: value["unread_count"] for key, value in durable()["rooms"].items()} == expected
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
    # The first registered worktree is the main checkout. Linked worktrees
    # build their own disposable bus in target/debug, just as the gate does.
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    worktrees = subprocess.check_output(
        ["git", "-C", str(ROOT), "worktree", "list", "--porcelain", "-z"], env=env,
    )
    main_checkout = Path(os.fsdecode(worktrees.split(b"\0", 1)[0].removeprefix(b"worktree ")))
    live_binary = main_checkout / "target/debug/bus"
    assert not (binary == live_binary.resolve()
                or (live_binary.exists() and binary.samefile(live_binary))), "live Bus binary is forbidden"
    with tempfile.TemporaryDirectory(prefix="br-restart-", dir="/private/tmp") as directory:
        root = Path(directory)
        scenario(binary, root / "claude", "claude", True)
        scenario(binary, root / "codex", "codex", False)


if __name__ == "__main__":
    main()
