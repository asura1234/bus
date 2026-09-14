#!/usr/bin/env python3
"""Remove severity and priority labels from review Markdown."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence


_RANK = r"(?:blocker|critical|major|minor|high|medium|low|p[0-3])"
_DECORATED_RANK = rf"(?:{_RANK}|(?:high|medium|low)\s+(?:severity|priority))"
_SEPARATOR = r"(?:\s*(?::|[-–—])\s*)"

_METADATA_LINE = re.compile(
    rf"^\s*(?:[-*+]\s+)?(?:\*\*)?\s*"
    rf"(?:severity(?:\s+level)?|priority)\s*(?::|[-–—])\s*"
    rf"(?:\*\*)?\s*\[?\s*{_RANK}\s*\]?\s*(?:\*\*)?[.!]?\s*$",
    re.IGNORECASE,
)
_STRUCTURAL_PREFIX = re.compile(r"^(?P<lead>\s*(?:(?:#{1,6}|[-*+])\s+)?)(?P<body>.*)$")
_PREFIX_PATTERNS = (
    re.compile(
        rf"^\*\*(?:\[{_DECORATED_RANK}\]|{_DECORATED_RANK})"
        rf"(?:\s*(?::|[-–—]))?\*\*(?:\s*(?::|[-–—]))?\s*",
        re.IGNORECASE,
    ),
    re.compile(rf"^\[{_DECORATED_RANK}\]\s*(?:{_SEPARATOR})?", re.IGNORECASE),
    re.compile(rf"^\({_DECORATED_RANK}\)\s*(?:{_SEPARATOR})?", re.IGNORECASE),
    re.compile(rf"^p[0-3]{_SEPARATOR}", re.IGNORECASE),
    re.compile(rf"^(?:blocker|critical|major|minor){_SEPARATOR}", re.IGNORECASE),
    re.compile(
        rf"^(?:severity(?:\s+level)?|priority){_SEPARATOR}{_RANK}{_SEPARATOR}",
        re.IGNORECASE,
    ),
)


def sanitize_review(text: str) -> tuple[str, int]:
    """Return review Markdown with label-like severity syntax removed."""

    sanitized_lines: list[str] = []
    removed = 0
    for line in text.splitlines(keepends=True):
        newline = "\n" if line.endswith("\n") else ""
        content = line[:-1] if newline else line
        if content.endswith("\r"):
            content = content[:-1]
            newline = "\r\n"

        if _METADATA_LINE.fullmatch(content):
            removed += 1
            continue

        match = _STRUCTURAL_PREFIX.fullmatch(content)
        assert match is not None
        lead = match.group("lead")
        body = match.group("body")
        while body:
            for pattern in _PREFIX_PATTERNS:
                prefix = pattern.match(body)
                if prefix is None:
                    continue
                body = body[prefix.end() :]
                removed += 1
                break
            else:
                break

        if not body and lead.strip():
            continue
        sanitized_lines.append(f"{lead}{body}{newline}")

    return "".join(sanitized_lines), removed


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--in-place",
        action="store_true",
        help="rewrite each review after removing severity labels",
    )
    mode.add_argument(
        "--check",
        action="store_true",
        help="report severity labels without modifying reviews",
    )
    parser.add_argument("review_files", nargs="+", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    dirty = False
    for review_file in args.review_files:
        try:
            source = review_file.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            sys.stderr.write(f"sanitize-review-severity error: {review_file}: {error}\n")
            return 2

        sanitized, removed = sanitize_review(source)
        noun = "tag" if removed == 1 else "tags"
        if args.check:
            if removed:
                dirty = True
                sys.stderr.write(
                    f"{review_file}: contains {removed} severity {noun}\n"
                )
        else:
            try:
                review_file.write_text(sanitized, encoding="utf-8", newline="")
            except OSError as error:
                sys.stderr.write(
                    f"sanitize-review-severity error: {review_file}: {error}\n"
                )
                return 2
            sys.stdout.write(
                f"{review_file}: sanitized {removed} severity {noun}\n"
            )

    return 1 if dirty else 0


if __name__ == "__main__":
    raise SystemExit(main())
