"""One owned coverage target per run; reports live outside its disposable data."""
from __future__ import annotations

import json
import os
import re
import shutil
import uuid
from pathlib import Path

from gate_process import cleanup_signals

EXTERNAL_RUN = "BUS_GATE_PROFILE_DIR"
MARKER = ".gate-coverage-owner.json"
MANIFEST = "active-coverage-run.json"


def profile_base(root: Path, env: dict) -> Path:
    if env.get("CARGO_LLVM_COV_TARGET_DIR"):
        return (root / env["CARGO_LLVM_COV_TARGET_DIR"]).resolve()
    return (root / env.get("CARGO_TARGET_DIR", "target") / "llvm-cov-target").resolve()


class Run:
    def __init__(self, root: Path, output: Path, lane: str):
        self.root = root.resolve()
        self.output = output
        self.lane = lane
        self.original = dict(os.environ)
        self.base = profile_base(root, self.original)
        self.path: Path | None = None
        self.retain = False
        self.external = bool(self.original.get(EXTERNAL_RUN)) and lane != "gate"
        self.manifest = output / MANIFEST

    def _validate(self, data: dict) -> Path:
        if not isinstance(data, dict) or data.get("root") != str(self.root):
            raise ValueError("coverage run belongs to a different checkout")
        if not all(isinstance(data.get(key), str) for key in ("path", "base")):
            raise ValueError("invalid coverage run ownership metadata")
        path = Path(data["path"])
        base = Path(data["base"])
        if (
            not path.is_absolute() or not base.is_absolute()
            or path.parent != base / ".gate-profile-runs"
            or not re.fullmatch(r"run-[0-9a-f]{32}", path.name)
            or path.is_symlink() or path.parent.is_symlink()
            or path.resolve() != path
        ):
            raise ValueError("invalid owned coverage run directory")
        owner = json.loads((path / MARKER).read_text(encoding="utf-8"))
        if owner != data:
            raise ValueError("coverage run ownership marker changed")
        return path

    def _remove(self, path: Path):
        with cleanup_signals():
            shutil.rmtree(path)
            # Never recursively delete the configured target or another run.
            for parent in (path.parent, path.parent.parent):
                try:
                    parent.rmdir()
                except OSError:
                    break

    def _load(self) -> dict:
        try:
            data = json.loads(self.manifest.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise ValueError("no active coverage run; run unit and integration first") from error
        self._validate(data)
        return data

    def __enter__(self):
        if self.external:
            self.path = Path(self.original[EXTERNAL_RUN])
            data = json.loads((self.path / MARKER).read_text(encoding="utf-8"))
            if self._validate(data) != self.base:
                raise ValueError("coverage run environment does not match its owner")
        elif self.lane in ("gate", "ci", "unit"):
            if self.manifest.exists():
                old = self._load()
                self._remove(Path(old["path"]))
                self.manifest.unlink()
            parent = self.base / ".gate-profile-runs"
            parent.mkdir(parents=True, exist_ok=True)
            if parent.is_symlink() or parent.resolve() != parent:
                raise ValueError("coverage run parent must not be a symlink")
            self.path = parent / ("run-" + uuid.uuid4().hex)
            self.path.mkdir(mode=0o700)
            data = {"root": str(self.root), "base": str(self.base), "path": str(self.path)}
            pending = self.output / (MANIFEST + ".tmp-" + self.path.name)
            try:
                (self.path / MARKER).write_text(json.dumps(data), encoding="utf-8")
                if self.lane != "gate":
                    self.output.mkdir(parents=True, exist_ok=True)
                    pending.write_text(json.dumps(data), encoding="utf-8")
                    pending.replace(self.manifest)
            except BaseException:
                pending.unlink(missing_ok=True)
                self._remove(self.path)
                raise
        else:
            data = self._load()
            if data["base"] != str(self.base):
                raise ValueError("coverage target changed; restart with the unit lane")
            self.path = Path(data["path"])
        env = {name: value for name, value in self.original.items()
               if not name.startswith(("BUS_", "HERDR_"))}
        env["CARGO_LLVM_COV_TARGET_DIR"] = str(self.path)
        if self.lane == "gate" or self.external:
            env[EXTERNAL_RUN] = str(self.path)
        os.environ.clear()
        os.environ.update(env)
        return self

    def finish(self, result: int):
        self.retain = result == 0 and self.lane in ("unit", "integration")

    def __exit__(self, exc_type, _error, _traceback):
        os.environ.clear()
        os.environ.update(self.original)
        if self.external or (exc_type is None and self.retain):
            return
        assert self.path is not None
        with cleanup_signals():
            data = json.loads((self.path / MARKER).read_text(encoding="utf-8"))
            self._validate(data)
            self._remove(self.path)
            if self.manifest.exists():
                current = json.loads(self.manifest.read_text(encoding="utf-8"))
                if current.get("path") == str(self.path):
                    self.manifest.unlink()
