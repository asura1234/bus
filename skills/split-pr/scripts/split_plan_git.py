"""Git object-space mechanics for split_plan.py; never touches a worktree."""

from __future__ import annotations

import os
import subprocess
from collections import Counter
from pathlib import Path

from split_plan_model import Part, Plan, PlanError, _require, closure, shared_commits


# --- git -------------------------------------------------------------------


def git(repo: Path, *args: str, check: bool = True, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        env={**os.environ, **env} if env else None,
    )
    if check and result.returncode != 0:
        raise PlanError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def rev(repo: Path, name: str) -> str:
    return git(repo, "rev-parse", "--verify", "--quiet", f"{name}^{{commit}}")


def is_ancestor(repo: Path, ancestor: str, descendant: str) -> bool:
    result = subprocess.run(
        ["git", "-C", str(repo), "merge-base", "--is-ancestor", ancestor, descendant],
        capture_output=True,
    )
    return result.returncode == 0


def range_commits(repo: Path, base: str, tip: str, *, merges: bool) -> list[str]:
    flag = "--merges" if merges else "--no-merges"
    out = git(repo, "rev-list", "--reverse", flag, f"{base}..{tip}")
    return out.split() if out else []


def commit_files(repo: Path, sha: str) -> list[str]:
    out = git(repo, "diff-tree", "--no-commit-id", "--name-only", "-r", sha)
    return out.splitlines()


def commit_numstat(repo: Path, sha: str) -> dict[str, tuple[int, int]]:
    out = git(repo, "diff-tree", "--no-commit-id", "--numstat", "-r", sha)
    stats: dict[str, tuple[int, int]] = {}
    for line in out.splitlines():
        added, deleted, path = line.split("\t", 2)
        # Binary files report `-` in numstat: count the file, not lines.
        stats[path] = (int(added) if added.isdigit() else 0, int(deleted) if deleted.isdigit() else 0)
    return stats


def tree_numstat(repo: Path, old: str, new: str) -> dict[str, tuple[int, int]]:
    out = git(repo, "diff", "--numstat", "--no-renames", old, new)
    stats: dict[str, tuple[int, int]] = {}
    for line in out.splitlines():
        added, deleted, path = line.split("\t", 2)
        stats[path] = (int(added) if added.isdigit() else 0, int(deleted) if deleted.isdigit() else 0)
    return stats


def part_size(repo: Path, plan: Plan, part: Part, position: dict[str, int]) -> tuple[list[str], int, int, bool]:
    """Size of the part's PR diff; the last item is True when it is a churn estimate.

    The PR shows the part's state against its parents' state, so commits that
    rewrite each other's lines count once. A part whose ancestry holds a
    hunk-split commit, or that does not replay cleanly, falls back to summing
    each commit's churn.
    """
    shared = shared_commits(plan)
    ancestry = closure(plan, part)
    stats: dict[str, tuple[int, int]] | None = None
    if not any(sha in shared for pid in ancestry for sha in plan.part(pid).commits):
        parent_commits = sorted(
            {sha for pid in ancestry if pid != part.id for sha in plan.part(pid).commits}, key=position.__getitem__
        )
        parent_state = replay(repo, plan.base_sha, parent_commits)
        part_state = replay(repo, parent_state, list(part.commits)) if parent_state is not None else None
        if parent_state is not None and part_state is not None:
            stats = tree_numstat(repo, parent_state, part_state)
    approx = stats is None
    if stats is None:
        stats = {}
        for sha in part.commits:
            for path, (plus, minus) in commit_numstat(repo, sha).items():
                old_plus, old_minus = stats.get(path, (0, 0))
                stats[path] = (old_plus + plus, old_minus + minus)
    added = sum(plus for plus, _ in stats.values())
    deleted = sum(minus for _, minus in stats.values())
    return sorted(stats), added, deleted, approx


def synthetic_commit(repo: Path, tree: str, parents: list[str], message: str) -> str:
    args = ["commit-tree", tree, "-m", message]
    for parent in parents:
        args += ["-p", parent]
    ident = {
        "GIT_AUTHOR_NAME": "split-pr",
        "GIT_AUTHOR_EMAIL": "split-pr@localhost",
        "GIT_COMMITTER_NAME": "split-pr",
        "GIT_COMMITTER_EMAIL": "split-pr@localhost",
    }
    return git(repo, *args, env=ident)


def merge_tree(repo: Path, ours: str, theirs: str, merge_base: str | None) -> str | None:
    """Return the merged tree, or None on conflict. Never touches a worktree."""
    args = ["merge-tree", "--write-tree", "--no-messages"]
    if merge_base is not None:
        args.append(f"--merge-base={merge_base}")
    result = subprocess.run(["git", "-C", str(repo), *args, ours, theirs], capture_output=True, text=True)
    if result.returncode == 0:
        return result.stdout.split()[0]
    if result.returncode == 1:
        return None
    raise PlanError(f"git merge-tree failed: {result.stderr.strip()}")


def replay(repo: Path, onto: str, commits: list[str]) -> str | None:
    """Cherry-pick `commits` onto `onto` in object space; None on conflict."""
    state = onto
    for sha in commits:
        tree = merge_tree(repo, state, sha, f"{sha}^")
        if tree is None:
            return None
        state = synthetic_commit(repo, tree, [state], f"probe {sha}")
    return state


def check_against_git(repo: Path, plan: Plan) -> None:
    _require(is_ancestor(repo, plan.base_sha, plan.source_sha), "base.sha is not an ancestor of source.sha")
    merges = range_commits(repo, plan.base_sha, plan.source_sha, merges=True)
    _require(not merges, f"source range contains merge commits {merges}; linearize the source first")
    in_range = range_commits(repo, plan.base_sha, plan.source_sha, merges=False)
    position = {sha: i for i, sha in enumerate(in_range)}
    assigned = [sha for p in plan.parts for sha in p.commits]
    for sha in set(assigned) | set(plan.left_on_source):
        _require(sha in position, f"commit {sha} is not in base..source")
    overlap = set(assigned) & set(plan.left_on_source)
    _require(not overlap, f"commits both assigned and left on source: {sorted(overlap)}")
    missing = [sha for sha in in_range if sha not in set(assigned) | set(plan.left_on_source)]
    _require(not missing, f"source commits not assigned to any part or left_on_source: {missing}")
    for part in plan.parts:
        ordered = sorted(part.commits, key=position.__getitem__)
        _require(list(part.commits) == ordered, f"part `{part.id}` commits must be in source order")
        result = subprocess.run(["git", "check-ref-format", "--branch", part.branch], capture_output=True)
        _require(result.returncode == 0, f"part `{part.id}` branch `{part.branch}` is not a valid branch name")


def change_balance(repo: Path, old: str, new: str) -> Counter[tuple[bytes, bytes, bytes]]:
    """Per (path, kind, value) count gained from `old` to `new`; kind is `line` or `mode`.

    These counts telescope along any chain of commits, so the parts' own diffs
    sum to the split's total exactly when no hunk or tree entry sits in two parts.
    Adding or deleting an empty file and chmod produce no text lines and are seen
    only through the mode counts (a missing side counts as `000000`).
    """
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "diff",
            "-U0",
            "--no-renames",
            "--text",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            old,
            new,
        ],
        capture_output=True,
    )
    if result.returncode != 0:
        raise PlanError(f"git diff {old[:12]} {new[:12]} failed: {result.stderr.decode(errors='replace').strip()}")
    balance: Counter[tuple[bytes, bytes, bytes]] = Counter()
    path = b""
    in_hunk = False
    for line in result.stdout.split(b"\n"):
        if line.startswith(b"diff --git "):
            in_hunk = False
        elif not in_hunk and line.startswith((b"--- ", b"+++ ")):
            if line[4:] != b"/dev/null":
                path = line[6:]
        elif line.startswith(b"@@"):
            in_hunk = True
        elif in_hunk and line.startswith(b"+"):
            balance[(path, b"line", line[1:])] += 1
        elif in_hunk and line.startswith(b"-"):
            balance[(path, b"line", line[1:])] -= 1
    raw = subprocess.run(
        ["git", "-C", str(repo), "diff", "--raw", "-z", "--no-renames", "--no-abbrev", old, new],
        capture_output=True,
    )
    if raw.returncode != 0:
        raise PlanError(f"git diff --raw {old[:12]} {new[:12]} failed: {raw.stderr.decode(errors='replace').strip()}")
    fields = raw.stdout.split(b"\0")
    for header, entry_path in zip(fields[0::2], fields[1::2]):
        old_mode, new_mode = header.lstrip(b":").split(b" ")[:2]
        if old_mode != new_mode:
            balance[(entry_path, b"mode", new_mode)] += 1
            balance[(entry_path, b"mode", old_mode)] -= 1
    return balance
