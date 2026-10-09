"""Disposable acceptance session lifecycle; importing never launches Bus or a provider."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
E2E = ROOT / "temp/e2e"
WORKSPACE = E2E / "workspace"
SCRUB_PREFIXES = ("BUS_", "HERDR_", "CLAUDE_CODE_")
SCRUB_NAMES = ("CLAUDECODE",)

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
    env.update(BUS_DATA_DIR=str(data_dir), TERM="xterm-256color")
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

    def __init__(self, binary, run_dir, env):
        self.binary, self.run_dir, self.env = binary, run_dir, env
        self.data = Path(env["BUS_DATA_DIR"])
        self.child = None
        self.master = None
        self.pump_thread = None

    def start(self):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
        tty_log = open(self.run_dir / "tui.log", "ab")
        self.child = subprocess.Popen(bus_argv(self.binary), stdin=slave, stdout=slave, stderr=slave,
                                      env=self.env, cwd=str(WORKSPACE), start_new_session=True)
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
            while pids := pids_holding([self.data, WORKSPACE]):
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
