"""Own gate child process groups so interruption finishes before profile cleanup."""
from __future__ import annotations

import contextlib
import os
import signal
import subprocess
import threading


class Interrupted(BaseException):
    def __init__(self, signum: int):
        self.signum = signum


@contextlib.contextmanager
def interrupt_signals():
    if threading.current_thread() is not threading.main_thread():
        yield
        return

    def interrupt(signum, _frame):
        raise Interrupted(signum)

    previous = {sig: signal.signal(sig, interrupt) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


@contextlib.contextmanager
def cleanup_signals():
    if threading.current_thread() is not threading.main_thread():
        yield
        return
    pending = []

    def defer(signum, _frame):
        pending.append(signum)

    previous = {sig: signal.signal(sig, defer)
                for sig in (signal.SIGINT, signal.SIGTERM)}
    completed = False
    try:
        yield
        completed = True
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        # A deferred interrupt must not replace a real teardown failure.
        if pending and completed:
            signum = pending[0]
            handler = previous[signum]
            if callable(handler):
                handler(signum, None)
            elif handler != signal.SIG_IGN:
                raise Interrupted(signum)


def signal_group(group: int, signum: int):
    try:
        os.killpg(group, signum)
    except ProcessLookupError:
        pass
    except PermissionError:
        # macOS cannot signal a zombie-only group. Inspection errors and live
        # members must retain a failure, rather than authorize cleanup.
        result = subprocess.run(("ps", "-axo", "pgid=,stat="),
                                capture_output=True, text=True, check=True)
        rows = [line.split() for line in result.stdout.splitlines()]
        if not rows or any(len(row) != 2 for row in rows):
            raise OSError(f"cannot inspect process group {group}")
        if any(int(pgid) == group and not state.startswith("Z") for pgid, state in rows):
            raise


class Children:
    def __init__(self, *, private_groups: bool = True):
        self.private_groups = private_groups
        self.active: set[subprocess.Popen] = set()
        self.lock = threading.Lock()
        self.stopped = False

    def start(self, *args, **kwargs):
        with self.lock:
            if self.stopped:
                raise RuntimeError("gate interrupted before child launch")
            if os.name == "posix" and self.private_groups:
                kwargs["start_new_session"] = True
            process = subprocess.Popen(*args, **kwargs)
            self.active.add(process)
            return process

    def finished(self, process):
        with self.lock:
            self.active.discard(process)

    def stop(self):
        with cleanup_signals():
            with self.lock:
                self.stopped = True
                active = list(self.active)
            failures = []
            for process in active:
                try:
                    self._terminate(process)
                except Exception as error:
                    failures.append(error)
            if failures:
                raise failures[0]

    def _terminate(self, process):
        group = os.name == "posix" and self.private_groups
        failures = []
        try:
            if group:
                signal_group(process.pid, signal.SIGTERM)
            elif os.name == "nt":
                subprocess.run(
                    ("taskkill", "/PID", str(process.pid), "/T", "/F"),
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    timeout=5, check=False,
                )
            elif process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
        except Exception as error:
            failures.append(error)
        try:
            if group:
                # A leader may have exited while descendants still hold its pipe.
                signal_group(process.pid, signal.SIGKILL)
        except Exception as error:
            failures.append(error)
        try:
            process.wait(timeout=2)
        except Exception as error:
            failures.append(error)
        finally:
            if process.returncode is not None:
                self.finished(process)
        if failures:
            raise failures[0]
