"""Run-owned coverage data beside a persistent instrumented Cargo build cache."""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import uuid
from pathlib import Path

from gate_process import cleanup_signals

EXTERNAL_RUN = "BUS_GATE_PROFILE_DIR"
PROFILE_DIR = "BUS_COVERAGE_PROFILE_DIR"
MARKER = ".gate-coverage-owner.json"
MANIFEST = "active-coverage-run.json"
MERGED = "merged.profdata"


def profile_base(root: Path, env: dict) -> Path:
    if env.get("CARGO_LLVM_COV_TARGET_DIR"):
        return (root / env["CARGO_LLVM_COV_TARGET_DIR"]).resolve()
    return (root / env.get("CARGO_TARGET_DIR", "target") / "llvm-cov-target").resolve()


def process_start(pid: int) -> str | None:
    """Return a process birth identity, None only for a confirmed dead PID.

    Lookup errors propagate: they must never authorize deleting a live run.
    """
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
        handle = kernel.OpenProcess(0x1000, False, pid)  # QUERY_LIMITED_INFORMATION
        if not handle:
            if ctypes.get_last_error() == 87:  # invalid PID
                return None
            raise OSError(ctypes.get_last_error(), "cannot inspect coverage owner")
        try:
            times = [wintypes.FILETIME() for _ in range(4)]
            kernel.GetProcessTimes.argtypes = (wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4))
            if not kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
                raise OSError(ctypes.get_last_error(), "cannot inspect coverage owner start")
            return str((times[0].dwHighDateTime << 32) | times[0].dwLowDateTime)
        finally:
            kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
            kernel.CloseHandle(handle)
    result = subprocess.run(
        ["ps", "-p", str(pid), "-o", "lstart="], capture_output=True, text=True,
        env={**os.environ, "LC_ALL": "C", "TZ": "UTC"},
    )
    if result.returncode == 1 and not result.stdout.strip() and not result.stderr.strip():
        return None
    if result.returncode or not result.stdout.strip():
        raise OSError("cannot inspect coverage owner start")
    return result.stdout.strip()


def owner_alive(data: dict) -> bool:
    return process_start(data["pid"]) == data["start"]


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
        if not all(isinstance(data.get(key), str) for key in ("path", "base", "start")) or (
            type(data.get("pid")) is not int or data["pid"] <= 0
            or not data["start"] or type(data.get("active")) is not bool
        ):
            raise ValueError("invalid coverage run ownership metadata")
        path, base = Path(data["path"]), Path(data["base"])
        if (
            not path.is_absolute() or base != self.base
            or path.parent != base / ".gate-profile-runs"
            or not re.fullmatch(r"run-[0-9a-f]{32}", path.name)
            or path.is_symlink() or path.parent.is_symlink()
            or path.resolve() != path or (path / MARKER).is_symlink()
        ):
            raise ValueError("invalid owned coverage run directory")
        if json.loads((path / MARKER).read_text(encoding="utf-8")) != data:
            raise ValueError("coverage run ownership marker changed")
        return path

    def _remove(self, path: Path):
        with cleanup_signals():
            # A report interrupted by SIGKILL may leave this hard-link lookup.
            # Remove it only when it is the exact inode owned by this run.
            index = self.base / (self.root.name + ".profdata")
            merged = path / MERGED
            if merged.exists() and index.exists() and index.samefile(merged):
                index.unlink()
            shutil.rmtree(path)
            try:
                path.parent.rmdir()
            except OSError:
                pass

    def _load(self) -> dict:
        try:
            data = json.loads(self.manifest.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise ValueError("no active coverage run; run unit and integration first") from error
        self._validate(data)
        return data

    def _write(self, data: dict, *, manifest: bool):
        assert self.path is not None
        marker = self.path / MARKER
        pending = marker.with_suffix(".tmp")
        pending.write_text(json.dumps(data), encoding="utf-8")
        pending.replace(marker)
        if manifest:
            self.output.mkdir(parents=True, exist_ok=True)
            pending = self.output / (MANIFEST + ".tmp-" + self.path.name)
            pending.write_text(json.dumps(data), encoding="utf-8")
            pending.replace(self.manifest)

    def _reap(self) -> bool:
        clear_flat = True
        # The manifest is intentionally not the source of crash recovery: gates
        # have their own markers, and multiple abandoned runs can coexist.
        for path in (self.base / ".gate-profile-runs").glob("run-*"):
            try:
                data = json.loads((path / MARKER).read_text(encoding="utf-8"))
                if self._validate(data) != path:
                    clear_flat = False
                    continue
                dead = not owner_alive(data)
            except (OSError, ValueError, KeyError, TypeError):
                clear_flat = False
                continue  # Unverifiable or foreign directories are never deleted.
            if dead:
                self._remove(path)
                if self.manifest.exists():
                    current = json.loads(self.manifest.read_text(encoding="utf-8"))
                    if current.get("path") == str(path):
                        self.manifest.unlink()
            elif data["active"]:
                clear_flat = False
        return clear_flat

    def _clear_flat(self):
        # Pre-lifecycle gates wrote here. Clean only flat data, once, before
        # collection; never recursively clean the persistent cache/live runs.
        for path in (*self.base.glob("*.profraw"), self.base / (self.root.name + ".profdata")):
            if path.is_file() or path.is_symlink():
                path.unlink()

    def __enter__(self):
        if self.external:
            self.path = Path(self.original[EXTERNAL_RUN])
            data = json.loads((self.path / MARKER).read_text(encoding="utf-8"))
            if self._validate(data) != self.path or not owner_alive(data):
                raise ValueError("coverage run environment does not match its owner")
        elif self.lane in ("gate", "ci", "unit"):
            clear_flat = self._reap()
            if self.manifest.exists():
                old = self._load()
                if old["active"] and owner_alive(old):
                    raise ValueError("coverage run is still held by a live owner")
                self._remove(Path(old["path"]))
                self.manifest.unlink()
            if clear_flat:
                self._clear_flat()
            parent = self.base / ".gate-profile-runs"
            parent.mkdir(parents=True, exist_ok=True)
            if parent.is_symlink() or parent.resolve() != parent:
                raise ValueError("coverage run parent must not be a symlink")
            self.path = parent / ("run-" + uuid.uuid4().hex)
            self.path.mkdir(mode=0o700)
            try:
                data = {"root": str(self.root), "base": str(self.base), "path": str(self.path),
                        "pid": os.getpid(), "start": process_start(os.getpid()), "active": True}
                self._write(data, manifest=self.lane != "gate")
            except BaseException:
                self._remove(self.path)
                raise
        else:
            data = self._load()
            if data["active"] and owner_alive(data):
                raise ValueError("coverage run is still held by a live owner")
            self.path = Path(data["path"])
            data.update(pid=os.getpid(), start=process_start(os.getpid()), active=True)
            self._write(data, manifest=True)
        env = {name: value for name, value in self.original.items()
               if not name.startswith(("BUS_", "HERDR_"))}
        env["CARGO_LLVM_COV_TARGET_DIR"] = str(self.base)
        env[PROFILE_DIR] = str(self.path)
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
        if self.external:
            return
        assert self.path is not None
        with cleanup_signals():
            data = json.loads((self.path / MARKER).read_text(encoding="utf-8"))
            self._validate(data)
            if exc_type is None and self.retain:
                data["active"] = False
                self._write(data, manifest=True)
                return
            self._remove(self.path)
            if self.manifest.exists():
                current = json.loads(self.manifest.read_text(encoding="utf-8"))
                if current.get("path") == str(self.path):
                    self.manifest.unlink()
