#!/usr/bin/env python3
"""Read and advance a room marker; see references/sitrep-format.md."""
import argparse
import datetime as dt
import hashlib
import json
import os
import re
import sys
import tempfile
from pathlib import Path


def utc(value):
    if not isinstance(value, str) or not re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", value):
        raise ValueError("timestamp must be UTC RFC 3339 with whole seconds")
    return dt.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)


def stamp(value):
    return value.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def validate_marker(value):
    if not isinstance(value, dict) or set(value) != {"timestamp", "items"}:
        raise ValueError("marker must have exactly timestamp and items")
    utc(value["timestamp"])
    items = value["items"]
    if not isinstance(items, list) or any(not isinstance(i, str) or not i.strip() or "\n" in i for i in items):
        raise ValueError("items must be nonempty single-line identifiers")
    return value


def load_marker(path):
    if path.is_symlink():
        raise ValueError("marker must not be a symlink")
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return None, None
    return validate_marker(json.loads(raw)), hashlib.sha256(raw).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("read", "write"))
    parser.add_argument("--main", required=True, type=Path)
    parser.add_argument("--room", required=True)
    parser.add_argument("--now", help="fixed UTC time for tests")
    args = parser.parse_args()
    try:
        if not args.main.is_absolute() or not args.main.is_dir() or not re.fullmatch(r"0|[1-9]\d*", args.room):
            raise ValueError("main must be an existing absolute directory and room a numeric ID")
        main_dir = args.main.resolve()
        path = main_dir / "temp" / ".bus" / f"sitrep-{args.room}.json"
        if not path.parent.resolve().is_relative_to(main_dir):
            raise ValueError("marker directory escapes main worktree")
        now = utc(args.now) if args.now else dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
        marker, revision = load_marker(path)
        if args.action == "read":
            since = utc(marker["timestamp"]) if marker else now - dt.timedelta(hours=24)
            if since > now:
                raise ValueError("marker timestamp is in the future")
            result = {"since": stamp(since), "cutoff": stamp(now), "initial": marker is None,
                      "items": marker["items"] if marker else [], "revision": revision}
        else:
            payload = json.load(sys.stdin)
            if not isinstance(payload, dict) or set(payload) != {"timestamp", "items", "revision"}:
                raise ValueError("write requires timestamp, items and revision")
            value = validate_marker({"timestamp": payload["timestamp"], "items": payload["items"]})
            if payload["revision"] != revision:
                raise ValueError("marker changed since read")
            when = utc(value["timestamp"])
            if when > now or (marker and when < utc(marker["timestamp"])):
                raise ValueError("marker timestamp cannot move backwards or into the future")
            path.parent.mkdir(parents=True, exist_ok=True)
            if not path.parent.resolve().is_relative_to(main_dir):
                raise ValueError("marker directory escapes main worktree")
            fd, name = tempfile.mkstemp(prefix=".sitrep-", dir=path.parent)
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as out:
                    json.dump(value, out, ensure_ascii=False, indent=2)
                    out.write("\n")
                os.replace(name, path)
            finally:
                if os.path.exists(name):
                    os.unlink(name)
            result = {"saved": str(path), "timestamp": value["timestamp"]}
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except (OSError, ValueError, TypeError) as error:
        print(f"sitrep marker: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
