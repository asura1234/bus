"""Immediate delivery through an actual scratch native server + TUI restart.

Only the provider is a stand-in. Restore, PTYs, hooks, coordinator and CLI are real.
Run explicitly, or through the dev-tools integration target; never at import time.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.acceptance.harness_test import Bus  # noqa: E402


def wait_initial_room(bus):
    """Control can answer before the TUI queues its first work room."""
    deadline = time.monotonic() + 30
    while True:
        state = bus.cli("state")
        if any(room["name"] == "bus" for room in state["rooms"]):
            return
        if time.monotonic() >= deadline:
            raise AssertionError(f"initial work room was not seeded: {state['rooms']}")
        time.sleep(0.05)


def build_identity(binary):
    output = subprocess.check_output([str(binary), "--identity"], text=True)
    return json.loads(output)["sha"]


def scenario(binary, root, whole_session=False, unhealthy=False, replacement_binary=None):
    replacement_binary = replacement_binary or binary
    initial_sha = build_identity(binary)
    replacement_sha = build_identity(replacement_binary)
    root.mkdir(parents=True, exist_ok=True)
    work, home, fake_bin = (root / name for name in ("work", "home", "bin"))
    for path in (work, home, fake_bin):
        path.mkdir()
    for name in ("claude", "open", "osascript", "terminal-notifier", "afplay"):
        (fake_bin / name).symlink_to(binary)
    env = {"PATH": f"{fake_bin}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home),
           "BUS_DATA_DIR": str(root / "data"), "BUS_TUI_FAKE_AGENT": "1",
           "TERM": "xterm-256color", "SHELL": "/bin/bash",
           "XDG_CONFIG_HOME": str(root / "xdg"), "BUS_LOG": "bus=debug",
           "BUS_TUI_EFFECTS": str(root / "effects.jsonl")}
    bus = Bus(binary, root, env, workspace=work)
    evidence = {}
    try:
        bus.start()
        wait_initial_room(bus)
        if whole_session:
            os.write(bus.master, b"restart-provider-draft")
            deadline = time.monotonic() + 30
            while not any(path.exists() and "restart-provider-draft" in path.read_text()
                          for path in (bus.data / "state.json", bus.data / "state.journal.json")):
                assert time.monotonic() < deadline, "scratch draft did not reach storage"
                time.sleep(0.05)
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
        initial_state = bus.cli("state")
        initial_build = initial_state["build"]
        assert initial_build["client"]["binary"] == str(binary), initial_build
        assert initial_build["server"]["binary"] == str(binary), initial_build
        assert initial_build["client"]["sha"] == initial_sha, initial_build
        assert initial_build["server"]["sha"] == initial_sha, initial_build
        evidence["initial_build"] = initial_build
        old_pid = bus.child.pid
        replacement = root / "bus-replacement"
        shutil.copy2(replacement_binary, replacement)
        if whole_session:
            if unhealthy:
                replacement.write_text("#!/bin/sh\nif [ \"$1\" = --identity ]; then\n"
                    f"echo '{{\"sha\":\"{'b' * 40}\",\"providers\":[\"claude\"]}}'\nelse exit 42; fi\n")
            call = subprocess.run([str(binary), "--dev", "restart", "--binary", str(replacement), "--confirm"],
                                  cwd=work, env=env, capture_output=True, text=True, timeout=30)
            assert call.returncode == 0, (call.stdout, call.stderr)
            assert json.loads(call.stdout)["stage"] == "handoff", call.stdout
            log = bus.data / "restarts/log.jsonl"
            deadline = time.monotonic() + 60
            while not (log.exists() and log.read_text().strip()):
                assert time.monotonic() < deadline, "whole-session outcome missing"
                time.sleep(0.05)
            outcome = json.loads(log.read_text().splitlines()[-1])
            assert outcome["outcome"] == ("rolled_back" if unhealthy else "restarted"), outcome
            tui = json.loads((bus.data / "restarts/tui.json").read_text())
            assert tui["process"]["pid"] != old_pid
            assert "restart-provider-draft" in (bus.data / "state.json").read_text()
            evidence.update(outcome=outcome, replacement_tui=tui)
        else:
            bus.stop()
            native_log = open(root / "native-replacement.log", "wb")
            native_process = subprocess.Popen([str(replacement), "server"], cwd=work,
                                              env=dict(env, HERDR_SESSION="bus", BUS_DEV="1"),
                                              stdout=native_log, stderr=subprocess.STDOUT,
                                              start_new_session=True)
            socket = bus.data / "herdr-config/sessions/bus/herdr.sock"
            stamp = bus.data / "restarts/build.json"
            deadline = time.monotonic() + 30
            while True:
                assert native_process.poll() is None, (root / "native-replacement.log").read_text()
                if socket.exists() and stamp.exists() and json.loads(stamp.read_text())["sha"] == replacement_sha:
                    break
                assert time.monotonic() < deadline, (root / "native-replacement.log").read_text()
                time.sleep(0.05)
            bus.start()
            assert native_process.poll() is None, (root / "native-replacement.log").read_text()
            native_log.close()
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
        final_state = bus.cli("state")
        final_build = final_state["build"]
        expected_final_sha = initial_sha if unhealthy else replacement_sha
        assert final_build["client"]["sha"] == (expected_final_sha if whole_session else initial_sha), final_build
        assert final_build["server"]["sha"] == expected_final_sha, final_build
        assert final_build["server"]["pid"] != initial_build["server"]["pid"], final_build
        if not whole_session:
            assert final_build["client"]["binary"] == str(binary), final_build
            assert final_build["server"]["binary"] == str(replacement), final_build
        evidence["final_build"] = final_build
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
    parser.add_argument("--replacement-binary", type=Path)
    parser.add_argument("--run-dir", type=Path)
    parser.add_argument("--whole-session", action="store_true")
    parser.add_argument("--unhealthy", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    replacement_binary = (args.replacement_binary or binary).resolve(strict=True)
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    entries = subprocess.check_output(["git", "-C", str(ROOT), "worktree", "list", "--porcelain", "-z"], env=env)
    live = Path(os.fsdecode(entries.split(b"\0", 1)[0].removeprefix(b"worktree "))) / "target/debug/bus"
    for candidate in (binary, replacement_binary):
        assert not (candidate == live.resolve() or (live.exists() and candidate.samefile(live))), "live binary forbidden"
    if args.run_dir:
        scenario(binary, args.run_dir.resolve(), args.whole_session, args.unhealthy, replacement_binary)
    else:
        with tempfile.TemporaryDirectory(prefix="bn-restart-", dir="/private/tmp") as directory:
            scenario(binary, Path(directory), args.whole_session, args.unhealthy, replacement_binary)


if __name__ == "__main__":
    main()
