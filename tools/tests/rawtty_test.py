"""Bounded raw-mode lifecycle checks on a private PTY; no terminal input is sent."""
import contextlib
import io
import os
import pty
import termios
from types import SimpleNamespace
import unittest
from unittest import mock

from tools.keyboard import capture_keys, rawtty


class RawTtyTests(unittest.TestCase):
    @contextlib.contextmanager
    def private_terminal(self):
        master, slave = pty.openpty()
        saved = termios.tcgetattr(slave)
        # macOS sets kernel-managed PENDIN when returning to canonical mode.
        # Include it in the initial snapshot so the kernel's transition is stable.
        saved[3] |= getattr(termios, "PENDIN", 0)
        termios.tcsetattr(slave, termios.TCSANOW, saved)
        saved = termios.tcgetattr(slave)
        try:
            with mock.patch.object(rawtty.sys, "stdin", SimpleNamespace(fileno=lambda: slave)):
                yield slave, saved
        finally:
            os.close(slave)
            os.close(master)

    def test_context_restores_mode_after_success(self):
        with self.private_terminal() as (fd, saved):
            with rawtty.RawMode():
                self.assertEqual(termios.tcgetattr(fd)[3] & (termios.ECHO | termios.ICANON), 0)
            self.assertEqual(termios.tcgetattr(fd), saved)

    def test_context_restores_mode_after_body_failure(self):
        with self.private_terminal() as (fd, saved):
            with self.assertRaisesRegex(RuntimeError, "capture failed"):
                with rawtty.RawMode():
                    raise RuntimeError("capture failed")
            self.assertEqual(termios.tcgetattr(fd), saved)

    def test_capture_keys_restores_mode_after_eof(self):
        with self.private_terminal() as (fd, saved):
            with mock.patch.object(capture_keys, "read_sequence", return_value=b""), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(capture_keys.main(), 0)
            self.assertEqual(termios.tcgetattr(fd), saved)

    def test_capture_keys_restores_mode_after_read_failure(self):
        with self.private_terminal() as (fd, saved):
            with mock.patch.object(capture_keys, "read_sequence", side_effect=OSError("read failed")), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaisesRegex(OSError, "read failed"):
                    capture_keys.main()
            self.assertEqual(termios.tcgetattr(fd), saved)


if __name__ == "__main__":
    unittest.main()
