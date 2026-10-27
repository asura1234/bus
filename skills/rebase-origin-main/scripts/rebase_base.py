#!/usr/bin/env python3
"""Resolve a caller's base or origin's symbolic default to an immutable commit."""
from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(["git", "-C", str(repo), *args], text=True, capture_output=True)
    if result.returncode:
        raise ValueError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout.strip()


def ensure_feature_branch(repo: Path, branch: str) -> None:
    if not branch:
        raise ValueError("a named feature branch is required")
    protected = {"main", "master"}
    result = subprocess.run(
        ["git", "-C", str(repo), "symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        text=True, capture_output=True,
    )
    if result.returncode not in (0, 1):
        raise ValueError(result.stderr.strip() or "cannot read origin/HEAD")
    if result.returncode == 0:
        ref = result.stdout.strip()
        if not ref.startswith("refs/remotes/origin/"):
            raise ValueError("origin/HEAD must point to an origin branch")
        protected.add(ref.removeprefix("refs/remotes/origin/"))
    if branch in protected:
        raise ValueError(f"refusing to rewrite or remediate protected branch: {branch}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--base", help="explicit ref or commit; defaults to origin's symbolic HEAD")
    parser.add_argument("--refresh-default", action="store_true", help="refresh origin/HEAD after fetching on a fresh invocation")
    parser.add_argument("--check-branch", help="refuse this named branch if it is protected")
    args = parser.parse_args()
    try:
        repo = Path(git(Path(args.repo), "rev-parse", "--show-toplevel"))
        if args.refresh_default:
            git(repo, "remote", "set-head", "origin", "--auto")
        if args.check_branch is not None:
            ensure_feature_branch(repo, args.check_branch)
        base = args.base
        if base is None:
            try:
                base = git(repo, "symbolic-ref", "refs/remotes/origin/HEAD")
            except ValueError as error:
                raise ValueError("origin's default branch is unavailable; run git remote set-head origin -a or pass --base") from error
            if not base.startswith("refs/remotes/origin/"):
                raise ValueError("origin/HEAD must point to an origin branch; pass --base")
        commit = git(repo, "rev-parse", "--verify", "--end-of-options", f"{base}^{{commit}}")
    except (OSError, ValueError) as error:
        print(f"rebase base resolution failed: {error}", file=sys.stderr)
        return 2
    print(commit)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
