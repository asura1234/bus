"""Shared terminal input, formatting and raw-mode lifecycle for keyboard tools."""
from __future__ import annotations

import re
import select
import sys
import termios
import tty


IDLE_TIMEOUT_S = 0.025


def to_hex(data: bytes) -> str:
    return data.hex()


def escaped(data: bytes) -> str:
    parts: list[str] = []
    for byte in data:
        if byte == 0x1B:
            parts.append("\\x1b")
        elif byte == 0x7F:
            parts.append("\\x7f")
        elif byte == 0x0D:
            parts.append("\\r")
        elif byte == 0x0A:
            parts.append("\\n")
        elif byte == 0x09:
            parts.append("\\t")
        elif 0x20 <= byte <= 0x7E:
            parts.append(chr(byte))
        else:
            parts.append(f"\\x{byte:02x}")
    return "".join(parts)


def sanitize(value: str) -> str:
    value = value.strip().lower()
    value = re.sub(r"[^a-z0-9._-]+", "-", value)
    value = re.sub(r"-+", "-", value).strip("-._")
    return value or "terminal"


def read_sequence() -> bytes:
    first = sys.stdin.buffer.read1(1)
    if not first:
        return b""

    chunks = [first]
    while True:
        ready, _, _ = select.select([sys.stdin], [], [], IDLE_TIMEOUT_S)
        if not ready:
            break
        chunk = sys.stdin.buffer.read1(1024)
        if not chunk:
            break
        chunks.append(chunk)
    return b"".join(chunks)


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
