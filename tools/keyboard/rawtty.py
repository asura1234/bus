"""Shared tty save/enter/restore lifecycle for interactive keyboard capture tools."""
from __future__ import annotations

import sys
import termios
import tty


def saved_mode(fd: int):
    return termios.tcgetattr(fd)


def enter_raw(fd: int) -> None:
    tty.setraw(fd)


def restore_mode(fd: int, saved) -> None:
    termios.tcsetattr(fd, termios.TCSADRAIN, saved)


class RawMode:
    def __enter__(self) -> "RawMode":
        self.fd = sys.stdin.fileno()
        self.old = saved_mode(self.fd)
        enter_raw(self.fd)
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        restore_mode(self.fd, self.old)
