#!/usr/bin/env python3
"""Check saved launch directories, then optionally remove a Git worktree."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys


UNREACHABLE = {
    "control_unavailable", "transport_error", "coordinator_unavailable",
    "response_timeout", "coordinator_timeout", "bus_down",
}
NEXT = (
    "Finish non-destructive cleanup, report the final result, and ask the orchestrator "
    "to delete those agents, then remove the worktree from an unaffected session."
)


def absolute_path(value: str) -> Path:
    if not isinstance(value, str) or not value.strip() or "\0" in value:
        raise ValueError("launch and worktree paths must be nonempty absolute paths")
    path = Path(value)
    if not path.is_absolute():
        raise ValueError(f"expected an absolute path: {value!r}")
    return path.resolve()


def live_holders(repo: Path, target: Path) -> list[tuple[int, str, Path]]:
    """Read the current roster; every retained agent holds its saved launch cwd."""
    try:
        done = subprocess.run(
            ["bus", "state"], cwd=repo, capture_output=True, text=True, timeout=15,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
        print(f"WARNING: Bus is unreachable ({exc}); continuing with existing worktree checks.",
              file=sys.stderr)
        return []

    try:
        state = json.loads(done.stdout)
    except json.JSONDecodeError as exc:
        raise ValueError("bus state returned invalid JSON; refusing removal") from exc
    if not isinstance(state, dict) or type(state.get("ok")) is not bool:
        raise ValueError("bus state has no valid ok field; refusing removal")
    if not state["ok"]:
        error = state.get("error")
        if isinstance(error, dict) and error.get("code") in UNREACHABLE:
            print(f"WARNING: Bus is unreachable ({error['code']}); "
                  "continuing with existing worktree checks.", file=sys.stderr)
            return []
        raise ValueError(f"bus state failed: {error!r}; refusing removal")
    if done.returncode != 0:
        raise ValueError("bus state reported success with a nonzero exit; refusing removal")
    result = state.get("result")
    if not isinstance(result, dict) or not isinstance(result.get("agents"), list):
        raise ValueError("bus state has no valid result.agents roster; refusing removal")

    holders = []
    seen = set()
    for agent in result["agents"]:
        if not isinstance(agent, dict):
            raise ValueError("bus state contains an invalid agent; refusing removal")
        agent_id, name = agent.get("id"), agent.get("name")
        if (type(agent_id) is not int or agent_id <= 0 or agent_id in seen
                or not isinstance(name, str) or not name.strip()):
            raise ValueError("bus state contains invalid or duplicate agent identity; refusing removal")
        seen.add(agent_id)
        # The CLI serializes the saved Agent.cwd as `cwd`, not `pwd`.
        cwd = absolute_path(agent.get("cwd"))
        if cwd.is_relative_to(target):
            holders.append((agent_id, name, cwd))
    return sorted(holders)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, help="absolute main checkout path")
    parser.add_argument("--worktree", required=True, help="absolute target worktree path")
    parser.add_argument("--session-cwd", required=True, help="saved launch cwd, never a tool/shell cwd")
    parser.add_argument("--check", action="store_true", help="check without removing anything")
    parser.add_argument("--force", action="store_true", help="pass one --force to Git after all guards")
    args = parser.parse_args()
    try:
        repo = absolute_path(args.repo)
        target = absolute_path(args.worktree)
        session = absolute_path(args.session_cwd)
        if target == repo:
            raise ValueError("Cannot remove the main worktree.")
        holders = live_holders(repo, target)
        if holders:
            names = "\n".join(f"  {name} ({agent_id}): {cwd}" for agent_id, name, cwd in holders)
            raise ValueError(f"Refusing to remove {target}: live Bus agents have launch directories "
                             f"inside it:\n{names}\n{NEXT}")
        if session.is_relative_to(target):
            raise ValueError(f"Refusing to remove {target}: it contains this agent session's launch "
                             f"directory ({session}). {NEXT}")
        if args.check:
            print(f"Launch-directory guard passed: {target}")
            return 0
        command = ["git", "-C", str(repo), "worktree", "remove"]
        if args.force:
            command.append("--force")
        done = subprocess.run([*command, str(target)], cwd=repo, check=False)
        return 0 if done.returncode == 0 else 1
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
