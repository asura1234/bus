"""Shared Git and command primitives for profile discovery and inference."""

from pathlib import Path
import subprocess


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(["git", "-C", str(repo), *args], capture_output=True)
    if result.returncode:
        raise ValueError(
            f"git {' '.join(args)}: {result.stderr.decode('utf-8', 'surrogateescape').strip()}"
        )
    return result.stdout.removesuffix(b"\n").decode("utf-8", "surrogateescape")


def command(argv: list[str], cwd: str = ".") -> dict:
    return {"cwd": cwd, "argv": argv}
