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
    try:
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        if pending:
            signum = pending[0]
            handler = previous[signum]
            if callable(handler):
                handler(signum, None)
            elif handler != signal.SIG_IGN:
                raise Interrupted(signum)


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
            for process in active:
                self._terminate(process)

    def _terminate(self, process):
        group = os.name == "posix" and self.private_groups
        try:
            if group:
                os.killpg(process.pid, signal.SIGTERM)
            elif os.name == "nt":
                subprocess.run(
                    ("taskkill", "/PID", str(process.pid), "/T", "/F"),
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    timeout=5, check=False,
                )
            elif process.poll() is None:
                process.terminate()
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
        finally:
            if group:
                # A leader may have exited while descendants still hold its pipe.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.wait(timeout=2)
            self.finished(process)
