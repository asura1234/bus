"""Idempotent JSONL append; never modifies the input evidence."""

import fcntl
import os
from pathlib import Path

from stats_schema import canonical, loads, validate


def brief_key(row):
    if (
        row["kind"] not in ("assignment", "handover")
        or row["data"]["message_id"] is None
    ):
        return None
    return (
        row["kind"],
        row["task"],
        row["role"],
        row["agent"]["id"],
        row["orchestrator"]["id"],
        row["data"]["room_id"],
        row["data"]["message_id"],
    )


def read_records(path):
    path = Path(path)
    if not path.exists():
        return []
    result = []
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        try:
            result.append(validate(loads(line)))
        except (ValueError, KeyError) as exc:
            raise ValueError(f"invalid {path}:{n}: {exc}") from exc
    if len({x["id"] for x in result}) != len(result):
        raise ValueError(f"invalid {path}: duplicate records")
    return result


def append(store, records):
    records = [validate(x) for x in records]
    store = Path(store).absolute()
    if any(p.is_symlink() for p in [store, *store.parents]):
        raise ValueError("invalid store: symlink path")
    store.mkdir(parents=True, exist_ok=True)
    lock_fd = os.open(store / ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(lock_fd, "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = store / "events.jsonl"
        if path.is_symlink():
            raise ValueError("invalid store: symlink events")
        existing = read_records(path)
        from stats_reviews import validate_reviews

        validate_reviews(existing + records)
        ids = {x["id"] for x in existing}
        briefs = {brief_key(x) for x in existing if brief_key(x) is not None}
        fresh = []
        for item in records:
            key = brief_key(item)
            if item["id"] not in ids and (key is None or key not in briefs):
                fresh.append(item)
                ids.add(item["id"])
                if key is not None:
                    briefs.add(key)
        from stats_phases import validate_phases

        validate_phases(existing + fresh)
        ended = [
            r["data"]["assignment_id"]
            for r in existing + fresh
            if r["kind"] == "assignment_end"
        ]
        if len(ended) != len(set(ended)):
            raise ValueError("invalid duplicate assignment end")
        if fresh:
            fd = os.open(
                path, os.O_APPEND | os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600
            )
            with os.fdopen(fd, "a", encoding="utf-8") as stream:
                if existing and path.read_bytes()[-1:] != b"\n":
                    raise ValueError("invalid store: incomplete final line")
                stream.write("".join(canonical(x) + "\n" for x in fresh))
                stream.flush()
                os.fsync(stream.fileno())
        return len(fresh)


def snapshot(store, value):
    """Publish a complete immutable snapshot before concurrent readers see it."""
    import tempfile
    from stats_schema import digest

    store = Path(store).absolute()
    if any(p.is_symlink() for p in [store, *store.parents]):
        raise ValueError("invalid store: symlink path")
    root = store / "raw"
    root.mkdir(parents=True, exist_ok=True)
    path = root / (digest(value) + ".json")
    content = canonical(value) + "\n"
    with tempfile.NamedTemporaryFile(
        mode="w", dir=root, encoding="utf-8", delete=False
    ) as stream:
        temporary = Path(stream.name)
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    try:
        try:
            os.link(temporary, path)
        except FileExistsError:
            if path.is_symlink() or path.read_text() != content:
                raise ValueError("invalid evidence snapshot collision") from None
    finally:
        temporary.unlink()
    return path
