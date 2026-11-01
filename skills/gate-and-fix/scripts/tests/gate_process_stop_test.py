"""Interruption must drain every child and retain real cleanup failures."""
import os
import signal
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import gate_process as processes  # noqa: E402
from gate_process import Children, interrupt_signals  # noqa: E402

IGNORE_TERM = (
    "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);"
    "print('ready',flush=True);time.sleep(60)"
)


def alive(pid):
    result = subprocess.run(("ps", "-p", str(pid), "-o", "stat="),
                            capture_output=True, text=True)
    if result.returncode == 1 and not result.stdout.strip() and not result.stderr.strip():
        return False
    result.check_returncode()
    return any(not state.startswith("Z") for state in result.stdout.split())


def start_child(children):
    child = children.start((sys.executable, "-c", IGNORE_TERM), stdout=subprocess.PIPE)
    assert child.stdout.readline() == b"ready\n"
    return child


def reap(child):
    child.kill()
    child.wait(timeout=5)
    child.stdout.close()


@unittest.skipUnless(os.name == "posix", "POSIX process-group and signal contract")
class ChildrenStopTest(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "macOS EPERM for zombie-only groups")
    def test_stop_kills_every_child_when_a_killed_leader_is_already_a_zombie(self):
        real_killpg = os.killpg

        def scheduled_after_exit(group, sig):
            # Hold the reviewer's scheduling window without adding host load.
            if sig == signal.SIGKILL:
                import time
                time.sleep(.3)
            real_killpg(group, sig)

        children = Children()
        first = start_child(children)
        second = start_child(children)
        try:
            with patch("gate_process.os.killpg", side_effect=scheduled_after_exit):
                children.stop()
            for child in (first, second):
                self.assertFalse(alive(child.pid), "child still running")
                self.assertEqual(child.returncode, -signal.SIGKILL, "child not reaped")
            self.assertFalse(children.active)
        finally:
            for child in (first, second):
                reap(child)

    def test_stop_waits_for_descendants_after_the_leader_has_exited(self):
        import threading

        script = (
            "import subprocess,sys;"
            f"p=subprocess.Popen((sys.executable,'-c',{IGNORE_TERM!r}),stdout=subprocess.PIPE);"
            "p.stdout.readline();print(p.pid,flush=True)"
        )
        children = Children()
        leader = children.start((sys.executable, "-c", script), stdout=subprocess.PIPE)
        descendant = int(leader.stdout.readline())
        leader.wait(timeout=5)
        signal_group = processes.signal_group
        pending = []

        def asynchronous_exit(group, signum):
            if signum == signal.SIGKILL:
                # A successful signal send does not mean the descendant has exited.
                timer = threading.Timer(.2, signal_group, (group, signum))
                pending.append(timer)
                timer.start()
            else:
                signal_group(group, signum)

        try:
            self.assertTrue(alive(descendant))
            with patch.object(processes, "signal_group", side_effect=asynchronous_exit):
                children.stop()
            self.assertFalse(alive(descendant), "stop returned while its descendant was alive")
        finally:
            for timer in pending:
                timer.join(timeout=5)
            signal_group(leader.pid, signal.SIGKILL)
            reap(leader)

    def test_stop_continues_after_failure_and_preserves_it_over_a_repeat_signal(self):
        for repeated_signal in (False, True):
            with self.subTest(repeated_signal=repeated_signal):
                children = Children(private_groups=False)
                first, second = start_child(children), start_child(children)
                failed, later = list(children.active)
                terminate = children._terminate

                def fail_one(child):
                    if child is failed:
                        if repeated_signal:
                            os.kill(os.getpid(), signal.SIGTERM)
                        raise PermissionError("injected cleanup failure")
                    terminate(child)

                try:
                    with interrupt_signals(), patch.object(children, "_terminate", side_effect=fail_one):
                        with self.assertRaisesRegex(PermissionError, "injected cleanup failure"):
                            children.stop()
                    self.assertFalse(alive(later.pid), "later child was never terminated")
                    self.assertEqual(later.returncode, -signal.SIGKILL)
                    self.assertEqual(children.active, {failed})
                finally:
                    reap(first)
                    reap(second)

    def test_permission_error_for_zombie_leader_with_live_descendant_is_not_ignored(self):
        script = (
            "import subprocess,sys,time;"
            f"p=subprocess.Popen((sys.executable,'-c',{IGNORE_TERM!r}),stdout=subprocess.PIPE);"
            "p.stdout.readline();print(p.pid,flush=True);time.sleep(60)"
        )
        leader = subprocess.Popen((sys.executable, "-c", script),
                                  stdout=subprocess.PIPE, start_new_session=True)
        descendant = int(leader.stdout.readline())
        real_killpg = os.killpg
        leader.kill()
        leader.stdout.read()  # EOF without wait retains the exited leader as a zombie.
        try:
            self.assertFalse(alive(leader.pid))
            self.assertTrue(alive(descendant))
            with patch("gate_process.os.killpg", side_effect=PermissionError("denied")):
                with self.assertRaises(PermissionError):
                    processes.signal_group(leader.pid, signal.SIGKILL)
            self.assertTrue(alive(descendant), "denied signal must not count as cleanup")
        finally:
            try:
                real_killpg(leader.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            reap(leader)

    def test_permission_error_does_not_hide_process_inspection_failure(self):
        child = subprocess.Popen((sys.executable, "-c", "pass"),
                                 stdout=subprocess.PIPE, start_new_session=True)
        try:
            child.stdout.read()  # retain a real exited child without reaping it
            for result in (OSError("ps unavailable"), subprocess.CompletedProcess([], 0, "", "")):
                with self.subTest(result=type(result).__name__):
                    with patch("gate_process.os.killpg", side_effect=PermissionError("denied")):
                        with patch("gate_process.subprocess.run", side_effect=(
                            result if isinstance(result, OSError) else None
                        ), return_value=result):
                            with self.assertRaises(OSError):
                                processes.signal_group(child.pid, signal.SIGKILL)
        finally:
            reap(child)

    def test_signal_error_still_reaps_and_finishes_an_exited_leader(self):
        children = Children()
        child = children.start((sys.executable, "-c", "pass"), stdout=subprocess.PIPE)
        child.stdout.read()
        try:
            with patch("gate_process.os.killpg", side_effect=PermissionError("denied")):
                with patch("gate_process.subprocess.run", side_effect=OSError("ps unavailable")):
                    with self.assertRaises(OSError):
                        children.stop()
            self.assertIsNotNone(child.returncode, "cleanup failure skipped reap")
            self.assertNotIn(child, children.active)
        finally:
            reap(child)
