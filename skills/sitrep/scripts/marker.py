#!/usr/bin/env python3
"""Compute sitrep state and save the default-branch marker; see sitrep-format.md."""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

SHA = re.compile(r"[0-9a-fA-F]{40}\Z")


def run(repo, args):
    try:
        return subprocess.run(args, cwd=repo, text=True, capture_output=True, check=False, timeout=60)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError(f"could not run {args[0]}: {error}") from error


def git(repo, *args, check=True):
    result = run(repo, ["git", *args])
    if check and result.returncode != 0:
        raise ValueError(f"git {args[0]} failed: {result.stderr.strip()}")
    return result


def main_worktree(repo):
    repo = repo.resolve()
    if not repo.is_absolute() or not repo.is_dir():
        raise ValueError("repo must be an existing absolute directory")
    common = Path(git(repo, "rev-parse", "--git-common-dir").stdout.strip())
    if not common.is_absolute():
        common = repo / common
    main = common.resolve().parent
    if not main.is_dir():
        raise ValueError("could not resolve the main worktree")
    return main


def fetch_head(repo):
    git(repo, "fetch", "origin")
    symbolic = git(repo, "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD", check=False)
    if symbolic.returncode == 0 and symbolic.stdout.strip().startswith("origin/"):
        default_ref = symbolic.stdout.strip()
    else:
        remote = git(repo, "remote", "show", "origin")
        match = re.search(r"(?m)^\s*HEAD branch: (\S+)\s*$", remote.stdout)
        if not match:
            raise ValueError("could not identify the origin default branch")
        default_ref = f"origin/{match.group(1)}"
    head = git(repo, "rev-parse", "--verify", f"{default_ref}^{{commit}}").stdout.strip()
    if not SHA.fullmatch(head):
        raise ValueError("default-branch head is not a full commit SHA")
    return default_ref, head


def open_prs(repo):
    result = run(repo, ["gh", "pr", "list", "--state", "open", "--author", "@me", "--limit", "1000", "--json", "number,title,url"])
    if result.returncode != 0:
        raise ValueError(f"gh could not list open PRs: {result.stderr.strip()}")
    try:
        pulls = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ValueError("gh returned invalid open PR JSON") from error
    if not isinstance(pulls, list):
        raise ValueError("gh open PR result must be an array")
    links = []
    for pull in pulls:
        if not isinstance(pull, dict) or not isinstance(pull.get("number"), int) or not isinstance(pull.get("title"), str) or not isinstance(pull.get("url"), str):
            raise ValueError("gh returned an invalid open PR entry")
        title = pull["title"].replace("\\", "\\\\").replace("[", "\\[").replace("]", "\\]")
        links.append(f"[#{pull['number']} {title}]({pull['url']})")
    return links


def marker_path(main):
    return main / "temp" / "sitrep" / "last-sitrep-commit"


def read_state(repo, start):
    main = main_worktree(repo)
    path = marker_path(main)
    if start is None:
        if path.is_symlink():
            raise ValueError("sitrep commit marker must not be a symlink")
        try:
            start = path.read_text(encoding="utf-8").strip()
        except FileNotFoundError as error:
            raise ValueError("no saved sitrep commit; provide --start from an explicit starting point") from error
    if not SHA.fullmatch(start):
        raise ValueError("sitrep starting point must be a full commit SHA")
    default_ref, head = fetch_head(main)
    ancestor = git(main, "merge-base", "--is-ancestor", start, default_ref, check=False)
    if ancestor.returncode != 0:
        raise ValueError("sitrep starting point is not an ancestor of the default branch")
    log = git(main, "log", "--format=%H%x00%s", f"{start}..{default_ref}").stdout
    finished = []
    for entry in log.splitlines():
        sha, subject = entry.split("\x00", 1)
        finished.append({"name": subject, "pr": f"landed {sha}"})
    return {"default_ref": default_ref, "head": head, "finished": finished, "open_prs": open_prs(main)}


def write_marker(repo):
    main = main_worktree(repo)
    _, head = fetch_head(main)
    path = marker_path(main)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".last-sitrep-commit-", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            output.write(head + "\n")
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return {"saved": str(path), "head": head}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("read", "write"))
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--start", help="explicit starting commit when no marker exists")
    args = parser.parse_args()
    try:
        result = read_state(args.repo, args.start) if args.action == "read" else write_marker(args.repo)
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except (OSError, ValueError, TypeError) as error:
        print(f"sitrep marker: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
