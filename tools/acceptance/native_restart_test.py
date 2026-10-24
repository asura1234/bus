"""Immediate delivery through an actual scratch native server + TUI restart.

Only the provider is a stand-in. Restore, PTYs, hooks, coordinator and CLI are real.
Run explicitly, or through the dev-tools integration target; never at import time.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.acceptance.harness_test import Bus  # noqa: E402


def scenario(binary, root):
    root.mkdir(parents=True, exist_ok=True)
    work, home, fake_bin = (root / name for name in ("work", "home", "bin"))
    for path in (work, home, fake_bin):
        path.mkdir()
    for name in ("claude", "open", "osascript", "terminal-notifier", "afplay"):
        (fake_bin / name).symlink_to(binary)
    env = {"PATH": f"{fake_bin}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home),
           "BUS_DATA_DIR": str(root / "data"), "BUS_TUI_FAKE_AGENT": "1",
           "TERM": "xterm-256color", "SHELL": "/bin/sh",
           "XDG_CONFIG_HOME": str(root / "xdg"), "BUS_LOG": "bus=debug",
           "BUS_TUI_EFFECTS": str(root / "effects.jsonl")}
    bus = Bus(binary, root, env, workspace=work)
    evidence = {}
    try:
        bus.start()
        bus.cli("agent", "add", "--room", "bus", "--name", "restart-agent",
                "--provider", "claude", "--pwd", str(work), "--consent-hooks")
        first = bus.cli("send", "--room", "bus", "--to", "restart-agent", "--text", "bootstrap")
        before = bus.cli("wait", "--message", str(first["message_id"]), "--timeout", "30")
        old = before["requests"][0]
        assert old["stage"] == "replied", before
        evidence["before"] = before
        agent = next(agent for agent in bus.cli("state")["agents"] if agent["name"] == "restart-agent")
        native_target = agent["runtime_identity"]["pane_id"]
        # The completed bootstrap already proves a working native process.
        original_native = bus.native_agent(native_target)
        assert original_native is not None, agent
        evidence["original_native"] = original_native
        old_pid = bus.child.pid
        bus.stop()
        # start() means control readiness only. No idle/provider wait is allowed
        # before this send: it must queue safely while native restore is pending.
        bus.start()
        assert bus.child.pid != old_pid
        sent = bus.cli("send", "--room", "bus", "--to", "restart-agent", "--text", "immediate-after-restart")
        evidence["send"] = sent
        after = bus.cli("wait", "--message", str(sent["message_id"]), "--timeout", "30")
        evidence["after"] = after
        new = after["requests"][0]
        assert new["stage"] == "replied", after
        assert new["reply"]["text"] == "fake reply: immediate-after-restart", after
        assert new["reply"]["provider_session_id"] == old["reply"]["provider_session_id"], (before, after)
        assert new["reply"]["provider_turn_id"] != old["reply"]["provider_turn_id"], (before, after)
        ready = bus.wait_native_ready(native_target)
        evidence["native_ready"] = ready
        assert ready["pane_id"] == original_native["pane_id"], evidence
        assert ready["agent_session"]["value"] == old["reply"]["provider_session_id"], evidence
        perf = root / "data/herdr-config/sessions/bus/perf/server.jsonl"
        boots = [record for line in perf.read_text().splitlines()
                 if (record := json.loads(line)).get("kind") == "run_start"]
        evidence["native_boots"] = boots
        assert len(boots) == 2 and boots[0]["pid"] != boots[1]["pid"], boots
        print(json.dumps({"result": "PASS", "before": old["reply"], "after": new["reply"]}), flush=True)
    except Exception as error:
        evidence["error"] = str(error)
        if "send" in evidence:
            evidence["last_status"] = bus.cli("message", "status", str(evidence["send"]["message_id"]))
        raise
    finally:
        (root / "evidence.json").write_text(json.dumps(evidence, indent=2))
        bus.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--run-dir", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    entries = subprocess.check_output(["git", "-C", str(ROOT), "worktree", "list", "--porcelain", "-z"], env=env)
    live = Path(os.fsdecode(entries.split(b"\0", 1)[0].removeprefix(b"worktree "))) / "target/debug/bus"
    assert not (binary == live.resolve() or (live.exists() and binary.samefile(live))), "live binary forbidden"
    if args.run_dir:
        scenario(binary, args.run_dir.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix="bn-restart-", dir="/private/tmp") as directory:
            scenario(binary, Path(directory))


if __name__ == "__main__":
    main()
