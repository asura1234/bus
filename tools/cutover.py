"""Verify a cutover's compiled revision and bytes without contacting Bus."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess


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
    args = parser.parse_args()
    try:
        receipt = (
            record(args.binary, args.head, args.manifest)
            if args.command == "record"
            else verify(args.binary, args.manifest)
        )
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Cutover verification failed: {error}\n")
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
