"""Disposable acceptance session lifecycle; importing never launches Bus or a provider."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import termios
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
E2E = ROOT / "temp/e2e"
WORKSPACE = E2E / "workspace"
SCRUB_PREFIXES = ("BUS_", "HERDR_", "CLAUDE_CODE_")
SCRUB_NAMES = ("CLAUDECODE", "CODEX_HOME", "CLAUDE_CONFIG_DIR")

class CliError(AssertionError):
    pass


def find_binary(explicit=None, root=ROOT):
    candidates = [Path(explicit)] if explicit else [root / "target/debug/bus"]
    for path in candidates:
        if path.is_file() and os.access(path, os.X_OK):
            return path.resolve()
    raise SystemExit("No Bus binary: build with cargo build, or pass --binary PATH (tried "
                     + ", ".join(map(str, candidates)) + ")")


def bus_argv(binary):
    return [str(binary), "--dev"]


def scrubbed_env(environ, data_dir):
    env = {k: v for k, v in environ.items() if not k.startswith(SCRUB_PREFIXES) and k not in SCRUB_NAMES}
    home = str(Path(data_dir).with_name(Path(data_dir).name + "-home"))
    env.update(BUS_DATA_DIR=str(data_dir), HOME=home, USERPROFILE=home, TERM="xterm-256color")
    env.setdefault("TEST_BUS_PROTECTED_HOME", environ.get("HOME", str(Path.home())))
    return env


def pids_holding(paths):
    pids = set()
    for path in paths:
        if Path(path).exists():
            out = subprocess.run(["lsof", "-t", "+D", str(path)], capture_output=True, text=True).stdout
            pids |= {int(pid) for pid in out.split()}
    return pids - {os.getpid()}


class Bus:
    """One throwaway Bus: a TUI client in a pty plus the dev CLI against its data dir."""

    def __init__(self, binary, run_dir, env, workspace=None):
        self.binary, self.run_dir, self.env = binary, run_dir, dict(env)
        self.env.setdefault("TEST_BUS_PROTECTED_HOME", os.environ.get("TEST_BUS_PROTECTED_HOME", str(Path.home())))
        self.workspace = Path(workspace) if workspace is not None else WORKSPACE
        self.data = Path(env["BUS_DATA_DIR"])
        self.child = None
        self.master = None
        self.pump_thread = None

    def prepare_skills(self):
        """Refuse the developer's HOME. Skill install is never part of launching a test Bus."""
        home = Path(self.env["HOME"])
        protected = Path(self.env["TEST_BUS_PROTECTED_HOME"])
        if not home.is_absolute() or home.resolve() == protected.resolve():
            raise AssertionError("test isolation refused: acceptance Bus needs its own HOME")

    def start(self):
        self.prepare_skills()
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
        tty_log = open(self.run_dir / "tui.log", "ab")
        self.child = subprocess.Popen(bus_argv(self.binary), stdin=slave, stdout=slave, stderr=slave,
                                      env=self.env, cwd=str(self.workspace), start_new_session=True)
        os.close(slave)
        master = self.master

        def pump():  # Keep draining the TUI so it never blocks on output.
            while True:
                try:
                    if select.select([master], [], [], 0.2)[0]:
                        chunk = os.read(master, 65536)
                        if not chunk:
                            break
                        tty_log.write(chunk[-4096:] if len(chunk) > 4096 else chunk)
                except OSError:
                    break
            tty_log.close()

        self.pump_thread = threading.Thread(target=pump, daemon=True)
        self.pump_thread.start()
        deadline = time.monotonic() + 30
        while True:
            try:
                return self.cli("state")
            except CliError:
                if self.child.poll() is not None or time.monotonic() > deadline:
                    raise
                time.sleep(0.5)

    def cli(self, *argv, timeout=60, row=None):
        command = [*map(str, argv)]
        started = time.monotonic()
        result = subprocess.run(bus_argv(self.binary) + command, capture_output=True, text=True,
                                env=self.env, timeout=timeout, check=False)
        try:
            response = json.loads(result.stdout)
        except ValueError:
            response = {"ok": False, "error": {"stdout": result.stdout[-2000:], "stderr": result.stderr[-2000:]}}
        entry = {"argv": command, "ok": bool(response.get("ok")),
                 "ms": round((time.monotonic() - started) * 1000)}
        if not entry["ok"]:
            entry["error"] = response.get("error")
        if row is not None:
            row["commands"].append(entry)
        if result.returncode or not response.get("ok"):
            raise CliError(response.get("error"))
        return response["result"]

    def native_agent(self, target):
        """Current native observation, never the coordinator's persisted idle state.

        Native metadata can also be restored before a PTY exists; only
        native_ready attests to the current runtime/provider process. The socket
        is strictly inside this fixture's data root.
        """
        path = self.data / "herdr-config/sessions/bus/herdr.sock"
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(3)
                stream.connect(str(path))
                request = {"id": "acceptance:native-ready", "method": "agent.list", "params": {}}
                stream.sendall(json.dumps(request).encode() + b"\n")
                response = json.loads(stream.makefile().readline())
        except (OSError, ValueError):
            return None
        return next((agent for agent in response.get("result", {}).get("agents", [])
                     if target in (agent.get("name"), agent.get("pane_id"), agent.get("terminal_id"))), None)

    def wait_native_ready(self, name, timeout=30):
        """Wait for a recreated, interactive native provider, not just control/UI readiness."""
        deadline = time.monotonic() + timeout
        while True:
            agent = self.native_agent(name)
            if (agent and agent.get("native_ready") and agent.get("interactive_ready")
                    and not agent.get("launch_pending")
                    and agent.get("agent_status") in ("idle", "done")):
                return agent
            if time.monotonic() >= deadline:
                raise AssertionError(f"native provider {name!r} did not become ready: {agent}")
            time.sleep(0.05)

    def stop(self):
        """Quit the TUI, stop the server, and kill anything still holding our directories."""
        report = {}
        try:
            self.cli("quit", timeout=15)
        except Exception as error:  # A dead Bus cannot quit; cleanup below still runs.
            report["quit_error"] = str(error)
        if self.child is not None:
            try:
                self.child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(self.child.pid, signal.SIGTERM)
                self.child.wait(timeout=5)
        # The headless server outlives the client; stop it so agents exit too. Killing is the fallback.
        stop = subprocess.run([str(self.binary), "stop"], env=self.env, capture_output=True, text=True,
                              timeout=20, check=False)
        report["stop"] = (stop.stdout.strip() if stop.returncode == 0
                          else f"exit {stop.returncode}: {stop.stderr.strip()}")
        killed = []
        for sig in (signal.SIGTERM, signal.SIGKILL):
            deadline = time.monotonic() + 5
            while pids := pids_holding([self.data, self.workspace]):
                if time.monotonic() > deadline:
                    for pid in pids:
                        try:
                            os.kill(pid, sig)
                            killed.append(pid)
                        except ProcessLookupError:
                            pass
                    time.sleep(1)
                    break
                time.sleep(0.5)
        if self.master is not None:
            try:
                os.close(self.master)
            except OSError:
                pass
            self.master = None
        report["killed"] = sorted(set(killed))
        report["leftover"] = sorted(pids_holding([self.data, WORKSPACE]))
        return report
