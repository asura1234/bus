"""Immediate delivery through an actual scratch native server + TUI restart.

Only the provider is a stand-in. Restore, PTYs, hooks, coordinator and CLI are real.
Run explicitly, or through the dev-tools integration target; never at import time.
"""
import argparse
from contextlib import contextmanager
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


@contextmanager
def restart_directory(run_dir, retain_failure, *, reply=False):
    """Only opt-in failures keep an exclusively claimed short restart root."""
    def check_sockets(root):
        cases = [root / "claude", root / "codex"] if reply else [root]
        for case in cases:
            for relative in ("herdr.sock", "herdr-client.sock"):
                socket = case / "data/herdr-config/sessions/bus" / relative
                if len(os.fsencode(socket)) > 96:
                    raise ValueError(f"restart socket path exceeds 96 encoded bytes: {socket}")

    if run_dir is None:
        if retain_failure:
            raise ValueError("--retain-failure requires --run-dir")
        with tempfile.TemporaryDirectory(prefix="br-restart-" if reply else "bn-restart-",
                                         dir="/private/tmp") as directory:
            root = Path(directory)
            check_sockets(root)
            yield root
        return
    root = Path(run_dir).absolute()
    check_sockets(root)
    if not retain_failure:
        yield root
        return
    if root.parent != Path("/private/tmp") or not root.name.startswith("bus-rf-"):
        raise ValueError("retained restart root must be a new bus-rf-* child of /private/tmp")
    root.mkdir(mode=0o700)  # Never adopt or delete an existing directory/symlink.
    owned = root.stat()
    try:
        yield root
    except BaseException:
        print(f"RESTART_FAILURE_RETAINED {root}", file=sys.stderr, flush=True)
        raise
    else:
        current = root.lstat()
        if (current.st_dev, current.st_ino) != (owned.st_dev, owned.st_ino) or root.is_symlink():
            raise RuntimeError("restart root identity changed; refusing cleanup")
        shutil.rmtree(root)


def capture_restart_failure(bus, root, evidence):
    """Best-effort read-only witness, after failure and before process cleanup."""
    began = time.monotonic()
    capture = {"phase": "before_cleanup", "errors": {}}
    evidence["failure_capture"] = capture

    def attempt(name, read):
        try:
            capture[name] = read()
            return capture[name]
        except Exception as error:
            capture["errors"][name] = str(error)
            return None

    state = attempt("state", lambda: bus.cli("state", timeout=3))
    if not evidence.get("bootstrap"):
        capture["bootstrap_error"] = evidence.get("error", "first message was not submitted")
    for name in ("bootstrap", "send"):
        sent = evidence.get(name)
        if sent:
            attempt(f"{name}_status", lambda sent=sent: bus.cli(
                "message", "status", str(sent["message_id"]), timeout=3))
    agents = state.get("agents", []) if isinstance(state, dict) else []
    for agent in agents[:1]:  # Each restart fixture creates exactly one agent.
        pane = agent.get("runtime_identity", {}).get("pane_id")
        if pane:
            attempt("native", lambda: bus.native_agent(pane))
        attempt("observe", lambda: bus.cli("agent", "observe", str(agent["id"]), timeout=3))

    # Do not follow fixture symlinks (provider stand-ins or any external path).
    snapshot = root / "failure-before-cleanup"
    paths = [root / "tui.log"]
    for directory, dirs, files in os.walk(bus.data, followlinks=False):
        dirs[:] = [name for name in dirs if not (Path(directory) / name).is_symlink()]
        paths.extend(Path(directory) / name for name in files)
    for source in paths:
        if source.is_symlink() or not source.is_file():
            continue
        relative = source.relative_to(root)

        def copy(source=source, relative=relative):
            destination = snapshot / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
            return str(relative)

        attempt(f"file:{relative}", copy)
    capture["capture_ms"] = round((time.monotonic() - began) * 1000)
    print(f"RESTART_FAILURE_CAPTURE {capture['capture_ms']}ms (post-failure diagnostics)",
          file=sys.stderr, flush=True)


def complete_restart_fixture(bus, root, evidence, original_error):
    """A diagnostic write must never skip cleanup or hide the scenario error."""
    try:
        try:
            (root / "evidence.json").write_text(json.dumps(evidence, indent=2))
        except Exception as error:
            if original_error is None:
                raise
            print(f"restart evidence write failed: {error}", file=sys.stderr)
    finally:
        try:
            bus.stop()
        except Exception as error:
            if original_error is None:
                raise
            print(f"restart cleanup failed: {error}", file=sys.stderr)


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


def scenario(binary, root, whole_session=False, unhealthy=False, retain_failure=False):
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
    original_error = None
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
        evidence["bootstrap"] = first
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
        if whole_session:
            replacement = root / "bus-replacement"
            shutil.copy2(binary, replacement)
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
    except BaseException as error:
        original_error = error
        evidence["error"] = str(error)
        if retain_failure:
            try:
                capture_restart_failure(bus, root, evidence)
            except Exception as diagnostic:
                evidence["capture_error"] = str(diagnostic)
        elif "send" in evidence:
            try:
                evidence["last_status"] = bus.cli("message", "status", str(evidence["send"]["message_id"]))
            except Exception as diagnostic:
                evidence["capture_error"] = str(diagnostic)
        raise
    finally:
        complete_restart_fixture(bus, root, evidence, original_error)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--run-dir", type=Path)
    parser.add_argument("--retain-failure", action="store_true")
    parser.add_argument("--whole-session", action="store_true")
    parser.add_argument("--unhealthy", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    entries = subprocess.check_output(["git", "-C", str(ROOT), "worktree", "list", "--porcelain", "-z"], env=env)
    live = Path(os.fsdecode(entries.split(b"\0", 1)[0].removeprefix(b"worktree "))) / "target/debug/bus"
    assert not (binary == live.resolve() or (live.exists() and binary.samefile(live))), "live binary forbidden"
    with restart_directory(args.run_dir, args.retain_failure) as directory:
        scenario(binary, directory, args.whole_session, args.unhealthy, args.retain_failure)


if __name__ == "__main__":
    main()
