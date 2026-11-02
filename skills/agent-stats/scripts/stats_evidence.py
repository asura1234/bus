"""Preserve source bytes before a disposable worktree or log changes."""

import hashlib
import os
import tempfile
from copy import deepcopy
from pathlib import Path

from stats_schema import digest


def preserve(rows, store):
    store = Path(store).absolute()
    if any(p.is_symlink() for p in [store, *store.parents]):
        raise ValueError("invalid evidence store path")
    root = store / "evidence"
    root.mkdir(parents=True, exist_ok=True)
    mapped = {}
    result = []
    for original in rows:
        row = deepcopy(original)
        for src in row["sources"]:
            path = Path(src["path"])
            raw = path.read_bytes()
            if hashlib.sha256(raw).hexdigest() != src["sha256"]:
                raise ValueError("evidence changed during collection: " + str(path))
            target = root / (src["sha256"] + path.suffix)
            with tempfile.NamedTemporaryFile(dir=root, delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
            try:
                try:
                    os.link(temporary, target)
                except FileExistsError:
                    if target.is_symlink() or target.read_bytes() != raw:
                        raise ValueError("invalid evidence collision") from None
            finally:
                temporary.unlink()
            src["locator"] = str(path) + "#" + src["locator"]
            src["path"] = str(target)
        if row["kind"] == "root_cause":
            row["data"]["run_ids"] = [mapped.get(x, x) for x in row["data"]["run_ids"]]
        if row["kind"] == "escape":
            row["data"]["claim_event"] = mapped[row["data"]["claim_event"]]
        row["id"] = digest({k: v for k, v in row.items() if k != "id"})
        mapped[original["id"]] = row["id"]
        result.append(row)
    return result
