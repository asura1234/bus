#!/usr/bin/env python3
"""Interactive verifier for suspicious terminal key captures.

Python 3 script using the adjacent rawtty.py helpers.
Prompts a short list of suspicious keys, captures each multiple times,
and writes a TSV file for easy comparison.

Usage:
    python3 verify_suspicious_keys.py
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

if __package__:
    from .rawtty import RawMode, to_hex, escaped, sanitize, read_sequence
else:
    from rawtty import RawMode, to_hex, escaped, sanitize, read_sequence

ABORT_CAPTURE = b"\x07"  # Ctrl+G
REPETITIONS = 2

SUSPICIOUS_KEYS: list[tuple[str, str]] = [
    ("home", "Press Home"),
    ("end", "Press End"),
    ("pageup", "Press Page Up"),
    ("pagedown", "Press Page Down"),
    ("shift+enter", "Press Shift+Enter"),
    ("alt+left", "Press Alt+Left Arrow"),
    ("alt+right", "Press Alt+Right Arrow"),
]


def prompt(text: str, default: str | None = None) -> str:
    suffix = f" [{default}]" if default else ""
    value = input(f"{text}{suffix}: ").strip()
    if not value and default is not None:
        return default
    return value


def choose_output_path(terminal_name: str) -> Path:
    stamp = time.strftime("%Y%m%d-%H%M%S")
    default_name = f"key-verify-{sanitize(terminal_name)}-{stamp}.tsv"
    raw = prompt("Output file path", default_name)
    return Path(raw).expanduser().resolve()


def capture_one(display: str, repetition: int) -> bytes | None:
    print()
    print(f"=== {display} (capture {repetition}/{REPETITIONS}) ===")
    print("Press the key now. Ctrl+G aborts the session.")
    sys.stdout.flush()
    with RawMode():
        data = read_sequence()
    if data == ABORT_CAPTURE:
        return None
    return data


def confirm_capture(data: bytes) -> str:
    print(f"Captured: hex={to_hex(data)} escaped={escaped(data)!r}")
    while True:
        choice = input("Enter=accept, r=retry, s=skip, q=quit: ").strip().lower()
        if choice in {"", "r", "s", "q"}:
            return choice
        print("Please enter one of: Enter, r, s, q")


def write_tsv(path: Path, rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as f:
        f.write("terminal\tkey\trepetition\tnotes\tbytes_hex\tescaped\n")
        for row in rows:
            f.write(
                "\t".join(
                    [
                        row["terminal"],
                        row["key"],
                        row["repetition"],
                        row["notes"],
                        row["bytes_hex"],
                        row["escaped"],
                    ]
                )
                + "\n"
            )


def main() -> int:
    print("Suspicious terminal key verifier")
    print("This captures a short list of keys multiple times for consistency checking.")
    print()

    terminal_name = prompt("Terminal name")
    if not terminal_name:
        print("Terminal name is required.", file=sys.stderr)
        return 1

    notes = prompt("Optional notes", "")
    output_path = choose_output_path(terminal_name)

    print()
    print(f"Terminal: {terminal_name}")
    print(f"Notes:    {notes or '-'}")
    print(f"Output:   {output_path}")
    print()
    input("Press Enter to start verifier...")

    rows: list[dict[str, str]] = []

    for key_id, display in SUSPICIOUS_KEYS:
        for repetition in range(1, REPETITIONS + 1):
            while True:
                data = capture_one(display, repetition)
                if data is None:
                    print("Aborted by Ctrl+G.")
                    write_tsv(output_path, rows)
                    print(f"Wrote partial capture to {output_path}")
                    return 1

                choice = confirm_capture(data)
                if choice == "r":
                    continue
                if choice == "s":
                    rows.append(
                        {
                            "terminal": terminal_name,
                            "key": key_id,
                            "repetition": str(repetition),
                            "notes": notes,
                            "bytes_hex": "",
                            "escaped": "SKIPPED",
                        }
                    )
                    break
                if choice == "q":
                    write_tsv(output_path, rows)
                    print(f"Wrote partial capture to {output_path}")
                    return 0

                rows.append(
                    {
                        "terminal": terminal_name,
                        "key": key_id,
                        "repetition": str(repetition),
                        "notes": notes,
                        "bytes_hex": to_hex(data),
                        "escaped": escaped(data),
                    }
                )
                break

    write_tsv(output_path, rows)
    print()
    print(f"Done. Wrote {len(rows)} rows to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
