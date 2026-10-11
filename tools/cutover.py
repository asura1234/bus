"""Verify a cutover's compiled revision and bytes without contacting Bus."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time


def inspect_binary(binary: Path, head: str) -> dict:
    if len(head) != 40 or any(char not in "0123456789abcdef" for char in head):
        raise ValueError("expected head must be a full lowercase commit SHA")
    binary = binary.resolve(strict=True)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    version = subprocess.check_output(
        [str(binary), "--version"], text=True, timeout=10
    ).strip()
    if not version.startswith("bus ") or not version.endswith(f"-dev.{head}"):
        raise ValueError(f"binary revision mismatch: expected {head}, got {version!r}")
    return {"head": head, "sha256": digest, "version": version}


def record(binary: Path, head: str, output: Path) -> dict:
    receipt = inspect_binary(binary, head)
    receipt["candidate"] = str(binary.resolve())
    output.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    return receipt


def verify(binary: Path, manifest: Path) -> dict:
    receipt = json.loads(manifest.read_text(encoding="utf-8"))
    observed = inspect_binary(binary, receipt["head"])
    if observed["sha256"] != receipt["sha256"] or observed["version"] != receipt["version"]:
        raise ValueError("installed binary differs from the verified candidate")
    return observed


def backup(source: Path, destination: Path) -> None:
    """Copy durable session data; Unix sockets are runtime endpoints, not data."""
    def sockets(directory, names):
        return [name for name in names
                if stat.S_ISSOCK((Path(directory) / name).lstat().st_mode)]

    shutil.copytree(source, destination, symlinks=True, ignore=sockets)


def processes() -> dict:
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,pgid=,stat=,lstart=,comm="], text=True, timeout=10
    )
    rows = {}
    for line in output.splitlines():
        parts = line.split(maxsplit=9)
        pid, parent, group, status = parts[:4]
        if not status.startswith("Z"):
            rows[int(pid)] = (int(parent), int(group), " ".join(parts[4:9]), parts[9])
    return rows


def session_roots(root: Path) -> set:
    # dev-control.lock is the lock name a Bus from before control.lock still holds.
    paths = [root / "coordinator.lock", root / "control.lock", root / "dev-control.lock", *root.rglob("*.sock")]
    paths = sorted({str(path) for path in paths if path.exists()})
    if not paths:
        return set()
    result = subprocess.run(["lsof", "-nP", "-t", *paths], capture_output=True, text=True, timeout=10)
    if result.returncode not in (0, 1) or result.stderr.strip():
        raise RuntimeError(f"lsof exited {result.returncode} inspecting session owners: {result.stderr.strip()}")
    return {int(pid) for pid in result.stdout.split()}


def extend(rows: dict, watched: dict, groups: dict, roots: set) -> set:
    alive = {pid for pid, born in watched.items() if pid in rows and rows[pid][2] == born}
    reused = {pid for pid, born in watched.items() if pid in rows and rows[pid][2] != born}
    # A process group ID is a PID too. Its leader's start time prevents the
    # numeric group being reused by a different session during the wait.
    owned_groups = {group for group, born in groups.items()
                    if group not in rows or rows[group][2] == born}
    candidates = (roots | alive) - reused
    while True:
        expanded = candidates | {pid for pid, (parent, group, _, _) in rows.items()
                                 if pid not in reused and (parent in candidates or group in owned_groups)}
        if expanded == candidates:
            break
        candidates = expanded
    for pid in candidates & rows.keys():
        watched[pid] = rows[pid][2]
        group = rows[pid][1]
        if group != os.getpgrp() and group in rows:
            groups[group] = rows[group][2]
    return {pid for pid, born in watched.items() if pid in rows and rows[pid][2] == born}


def snapshot(source: Path, manifest: Path) -> None:
    watched, groups = {}, {}
    rows = {}
    for _ in range(2):
        rows = processes()
        extend(rows, watched, groups, session_roots(source))
    if not watched:
        raise RuntimeError("Cannot identify this session's socket/lock owners. Nothing has been stopped.")
    manifest.write_text(json.dumps({"pids": watched, "groups": groups,
                                    "processes": {pid: rows[pid] for pid in watched if pid in rows}}, indent=2) + "\n")
    print(f"Watching {len(watched)} session processes; snapshot: {manifest}")


def wait(manifest: Path, timeout: float = 60) -> None:
    saved = json.loads(manifest.read_text())
    watched = {int(pid): born for pid, born in saved["pids"].items()}
    groups = {int(group): born for group, born in saved["groups"].items()}
    deadline = time.monotonic() + timeout
    while True:
        rows = processes()
        # Ownership was established before stop. Never inspect disappearing
        # sockets here or adopt a new process holding a replacement inode.
        alive = extend(rows, watched, groups, set())
        if not alive:
            print("This session's server, client, and tracked pane processes have exited.")
            return
        if time.monotonic() >= deadline:
            details = "\n".join(f"PID {pid}: PPID={rows[pid][0]} PGID={rows[pid][1]} "
                                f"started={rows[pid][2]} command={rows[pid][3]}" for pid in sorted(alive))
            manifest.with_suffix(".remaining.json").write_text(json.dumps(
                {pid: rows[pid] for pid in sorted(alive)}, indent=2) + "\n")
            raise RuntimeError(f"Timed out after {timeout:g} seconds waiting for session processes:\n{details}")
        time.sleep(0.25)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    capture = subparsers.add_parser("record")
    capture.add_argument("--binary", type=Path, required=True)
    capture.add_argument("--head", required=True)
    capture.add_argument("--manifest", type=Path, required=True)
    check = subparsers.add_parser("verify")
    check.add_argument("--binary", type=Path, required=True)
    check.add_argument("--manifest", type=Path, required=True)
    for command in ["backup", "snapshot", "wait"]:
        operation = subparsers.add_parser(command)
        operation.add_argument("--source", type=Path, required=True)
        if command == "backup":
            operation.add_argument("--destination", type=Path, required=True)
        else:
            operation.add_argument("--manifest", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "backup":
            backup(args.source, args.destination)
            return
        if args.command == "snapshot":
            snapshot(args.source, args.manifest)
            return
        if args.command == "wait":
            wait(args.manifest)
            return
        receipt = (
            record(args.binary, args.head, args.manifest)
            if args.command == "record"
            else verify(args.binary, args.manifest)
        )
    except (OSError, ValueError, KeyError, RuntimeError, shutil.Error, subprocess.SubprocessError) as error:
        parser.exit(1, f"Cutover {args.command} failed: {error}\n")
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
