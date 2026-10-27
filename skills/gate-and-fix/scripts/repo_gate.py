"""Select repository policy through the installed worktree scanner, never a stale cache."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys

SCANNER = Path(__file__).resolve().parents[2] / "worktree-new/scripts/repo_profile.py"


def read_policy(repo: Path) -> dict:
    """Consume the producer's fresh policy without creating target artifacts."""
    if not SCANNER.is_file():
        raise ValueError("repository profile scanner is not installed")
    result = subprocess.run(
        [sys.executable, str(SCANNER), "scan", "--repo", str(repo), "--no-cache"],
        cwd=repo, capture_output=True, text=True,
    )
    if result.returncode:
        raise ValueError(f"repo profile scan failed: {result.stderr.strip()}")
    try:
        policy = json.loads(result.stdout)["policy"]
        configured = policy["gate_command"]
        exact_head = policy["require_exact_head_gate"]
        if configured is not None and (not isinstance(configured, str) or not configured.strip()):
            raise ValueError("gate_command must be a nonempty string or null")
        if not isinstance(exact_head, bool):
            raise ValueError("require_exact_head_gate must be boolean")
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"invalid repo profile: {error}") from error
    return policy


def gate_policy(repo: Path, command: str | None) -> tuple[str, bool]:
    """Return the effective command and whether the profile opts into Bus gates."""
    configured = read_policy(repo)["gate_command"] if SCANNER.is_file() else None
    effective = command if command is not None else configured
    if not isinstance(effective, str) or not effective.strip():
        raise ValueError("no gate configured: set gate_command in .bus/repo.toml or pass --command")
    return effective, configured == "just ci" and effective == "just ci"


def command_argv(command: str) -> tuple[str, ...]:
    """Profile gate commands intentionally support shell syntax, as repo.toml specifies."""
    return ("cmd", "/d", "/s", "/c", command) if os.name == "nt" else ("sh", "-c", command)


def artifact_root(repo: Path, requested: str | None) -> Path:
    """Keep evidence out of source status without changing ignore configuration."""
    def git(*args: str) -> str:
        result = subprocess.run(["git", "-C", str(repo), *args], text=True, capture_output=True)
        if result.returncode:
            raise ValueError(result.stderr.strip() or "cannot resolve evidence metadata")
        return result.stdout.strip()

    common = Path(git("rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()
    fallback = common / "bus/gate-and-fix"
    if requested is None:
        return fallback
    requested_path = Path(requested).resolve()
    if requested_path.is_relative_to(common):
        return requested_path
    # A linked worktree's scanner/evidence must not dirty its main checkout either.
    for record in git("worktree", "list", "--porcelain").splitlines():
        if not record.startswith("worktree "):
            continue
        worktree = Path(record.removeprefix("worktree ")).resolve()
        if requested_path.is_relative_to(worktree):
            ignored = subprocess.run(
                ["git", "-C", str(worktree), "check-ignore", "--quiet", "--", str(requested_path)],
                capture_output=True,
            )
            if ignored.returncode not in (0, 1):
                raise ValueError("cannot determine whether the evidence root is ignored")
            return requested_path if ignored.returncode == 0 else fallback
    return requested_path
