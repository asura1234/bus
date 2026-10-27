"""Select repository policy through the installed worktree scanner, never a stale cache."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys

SCANNER = Path(__file__).resolve().parents[2] / "worktree-new/scripts/repo_profile.py"


def gate_policy(repo: Path, command: str | None) -> tuple[str, bool]:
    """Return the effective command and whether the profile opts into Bus gates."""
    configured = None
    if SCANNER.is_file():
        result = subprocess.run(
            [sys.executable, str(SCANNER), "scan", "--repo", str(repo)],
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
    effective = command if command is not None else configured
    if not isinstance(effective, str) or not effective.strip():
        raise ValueError("no gate configured: set gate_command in .bus/repo.toml or pass --command")
    return effective, configured == "just ci" and effective == "just ci"


def command_argv(command: str) -> tuple[str, ...]:
    """Profile gate commands intentionally support shell syntax, as repo.toml specifies."""
    return ("cmd", "/d", "/s", "/c", command) if os.name == "nt" else ("sh", "-c", command)
