#!/usr/bin/env python3
"""Capture terminal key sequences in raw mode and print them as hex.

Usage:
    python3 tools/keyboard/capture_keys.py

Behavior:
- Switches stdin to raw mode
- Coalesces bytes until input goes idle for 20ms
- Prints each coalesced sequence as hex + escaped text
- Exit with Ctrl+G (0x07)

This is meant for collecting real terminal fixtures from Ghostty, GNOME Terminal,
kitty, iTerm2, Terminal.app, tmux, etc.
"""

from __future__ import annotations

import select
import sys

if __package__:
    from .rawtty import saved_mode, enter_raw, restore_mode, to_hex, escaped
else:
    from rawtty import saved_mode, enter_raw, restore_mode, to_hex, escaped

IDLE_TIMEOUT_S = 0.020
EXIT_BYTE = b"\x07"  # Ctrl+G


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


def main() -> int:
    fd = sys.stdin.fileno()
    old = saved_mode(fd)
    print("capture-keys: raw mode enabled", file=sys.stderr)
    print("capture-keys: press Ctrl+G to quit", file=sys.stderr)
    print("family\thex\tescaped", file=sys.stdout)
    sys.stdout.flush()

    try:
        enter_raw(fd)
        while True:
            data = read_sequence()
            if not data:
                return 0
            if data == EXIT_BYTE:
                print("capture-keys: exiting", file=sys.stderr)
                return 0
            print(f"captured\t{to_hex(data)}\t{escaped(data)}", file=sys.stdout)
            sys.stdout.flush()
    finally:
        restore_mode(fd, old)


if __name__ == "__main__":
    raise SystemExit(main())
