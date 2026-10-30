#!/usr/bin/env python3
"""Inspect initialized submodule work and resync only recorded gitlink commits."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile

from rebase_base import git
from repository_context import read_policy, repository_root


def paths(repo: Path) -> list[str]:
    if not (repo / '.gitmodules').is_file():
        return []
    result = subprocess.run(['git', '-C', str(repo), 'config', '-z', '--file', '.gitmodules',
                             '--get-regexp', r'^submodule\..*\.path$'], text=True, capture_output=True)
    if result.returncode not in (0, 1):
        raise ValueError(result.stderr.strip())
    found = []
    for record in result.stdout.split('\0'):
        if not record:
            continue
        path = record.split('\n', 1)[1]
        normalized = PurePosixPath(path)
        full = repo / path
        if normalized.is_absolute() or '..' in normalized.parts or full.is_symlink() or not full.resolve().is_relative_to(repo.resolve()):
            raise ValueError(f'unsafe submodule path: {path}')
        if (full / '.git').exists():
            found.append(path)
    return sorted(found)


def recorded_pin(repo: Path, path: str, before: str | None) -> str | None:
    if before is not None:
        fields = git(repo, 'ls-tree', '-z', before, '--', path).partition('\t')[0].split()
        return fields[2] if len(fields) == 3 and fields[:2] == ['160000', 'commit'] else None
    records = git(repo, 'ls-files', '--stage', '-z', '--', path).split('\0')
    pins = [fields[1] for row in records if row
            for fields in [row.partition('\t')[0].split()]
            if len(fields) == 3 and fields[0] == '160000' and fields[2] == '0']
    return pins[0] if len(pins) == 1 else None


def remote_ref_tips(remote: str) -> list[str] | None:
    result = subprocess.run(['git', 'ls-remote', remote], text=True, capture_output=True,
                            env={**os.environ, 'GIT_TERMINAL_PROMPT': '0'})
    if result.returncode:
        return None
    return sorted({fields[0] for line in result.stdout.splitlines()
                   if len(fields := line.split('\t')) == 2})


def local_commit(repo: Path, object_name: str) -> str | None:
    result = subprocess.run(['git', '-C', str(repo), 'rev-parse', '--verify',
                             f'{object_name}^{{commit}}'], text=True, capture_output=True)
    return result.stdout.strip() if result.returncode == 0 else None


def fetch_remote_tips(repo: Path, remote: str, tips: list[str], *, filtered: bool = True) -> tuple[bool, str]:
    args = ['git', '-C', str(repo), 'fetch', '--no-tags', '--no-write-fetch-head']
    if filtered:
        args.append('--filter=tree:0')
    result = subprocess.run([*args, remote, *tips], text=True, capture_output=True,
                            env={**os.environ, 'GIT_TERMINAL_PROMPT': '0'})
    return result.returncode == 0, result.stderr


def commits_reachable_from_tips(repo: Path, commits: list[str], tips: list[str]) -> set[str]:
    remote_commits = {commit for tip in tips if (commit := local_commit(repo, tip)) is not None}
    confirmed = set()
    for commit in commits:
        for tip in remote_commits:
            ancestor = subprocess.run(['git', '-C', str(repo), 'merge-base', '--is-ancestor', commit, tip],
                                      capture_output=True)
            if ancestor.returncode == 0:
                confirmed.add(commit)
                break
    return confirmed


def fetch_tips_with_fallback(repo: Path, url: str, tips: list[str]) -> bool:
    confirmed, error = fetch_remote_tips(repo, url, tips)
    if not confirmed and 'filter' in error.lower() and 'not our ref' not in error.lower():
        confirmed, error = fetch_remote_tips(repo, url, tips, filtered=False)
    return confirmed


def remote_confirmed_commits(repo: Path, commits: list[str]) -> set[str]:
    """Confirm ancestry from advertised remote refs; fail closed on remote errors."""
    commits = sorted(set(commits))
    if not commits:
        return set()
    remote = subprocess.run(['git', '-C', str(repo), 'remote', 'get-url', 'origin'],
                            text=True, capture_output=True)
    if remote.returncode or not remote.stdout.strip():
        return set()
    url = remote.stdout.strip()
    tips = remote_ref_tips(url)
    if tips is None:
        return set()
    if git(repo, 'rev-parse', '--is-shallow-repository') == 'true':
        with tempfile.TemporaryDirectory(prefix='bus-child-remote-') as directory:
            probe = Path(directory)
            init = subprocess.run(['git', 'init', '-q', '--bare', directory], capture_output=True)
            if init.returncode:
                return set()
            objects = Path(git(repo, 'rev-parse', '--absolute-git-dir')) / 'objects'
            (probe / 'objects/info/alternates').write_text(f'{objects}\n')
            if not fetch_tips_with_fallback(probe, url, tips):
                return set()
            return commits_reachable_from_tips(probe, commits, tips)
    missing = [tip for tip in tips if local_commit(repo, tip) is None]
    if missing and not fetch_tips_with_fallback(repo, url, missing):
        return set()
    return commits_reachable_from_tips(repo, commits, tips)


def remote_has_commit(repo: Path, commit: str) -> bool:
    return commit in remote_confirmed_commits(repo, [commit])


def unpublished_child_work(repo: Path, pin: str, head: str) -> str:
    # Local branches and detached HEAD are work sources. Only remote-tracking
    # refs and SHAs confirmed by the child origin are publication evidence.
    candidates = git(repo, 'log', '--format=%H %s', '--branches', 'HEAD',
                     '--not', '--remotes').splitlines()
    candidate_shas = {line.split(' ', 1)[0] for line in candidates}
    # Other local branch tips remain candidates but are not accepted merely
    # because their (possibly dangling) objects can be fetched by SHA.
    tips = [commit for commit in {pin, head} if commit in candidate_shas]
    confirmed = remote_confirmed_commits(repo, tips)
    if confirmed:
        # A served tip proves all of its reachable ancestors are published too.
        candidates = git(repo, 'log', '--format=%H %s', '--branches', 'HEAD',
                         '--not', '--remotes', *sorted(confirmed)).splitlines()
    return '\n'.join(candidates)


def inspect(repo: Path, prefix: str = '', before: str | None = None) -> list[dict]:
    states = []
    for path in paths(repo):
        full = repo / path
        if repository_root(full) != full.resolve():
            raise ValueError(f'submodule Git root mismatch: {path}')
        previous = recorded_pin(repo, path, before) if before is not None else None
        pin = recorded_pin(repo, path, None) or previous
        if pin is None:
            raise ValueError(f'unresolved or missing gitlink: {path}')
        head = git(full, 'rev-parse', 'HEAD')
        states.append({'path': prefix + path, 'pin': pin, 'head': head,
                       'moved': head != pin, 'unrecorded': head not in (pin, previous),
                       'unpushed': unpublished_child_work(full, pin, head),
                       'dirty': git(full, 'status', '--porcelain', '--untracked-files=all', '--ignore-submodules=all')})
        states.extend(inspect(full, prefix + path + '/', previous))
    return states


def sync(repo: Path, before: str | None = None) -> list[dict]:
    policy = read_policy(repo)['submodule_update']
    if policy not in ('recursive', 'none'):
        raise ValueError('unsupported submodule_update policy')
    states = inspect(repo, before=before)
    if any(state['dirty'] or state['unpushed'] or state['unrecorded'] for state in states):
        raise ValueError('submodule has uncommitted, unpushed or unrecorded work; refusing resync: ' + json.dumps(states))
    initialized = paths(repo)
    if policy == 'recursive' and initialized:
        git(repo, 'submodule', 'update', '--checkout', '--recursive', '--', *initialized)
    after = inspect(repo)
    if any(state['dirty'] or state['moved'] or state['unpushed'] for state in after):
        raise ValueError('submodules differ from recorded gitlinks or have unpushed work; no push: ' + json.dumps(after))
    return after


def take_base_gitlink(repo: Path, path: str) -> dict:
    # The caller's target policy decides whether taking the base is appropriate.
    stages = [row.split('\t', 1)[0].split() for row in git(repo, 'ls-files', '-u', '--', path).splitlines()]
    ours = [fields[1] for fields in stages if len(fields) == 3 and fields[0] == '160000' and fields[2] == '2']
    if len(ours) != 1:
        raise ValueError(f'no unique stage-2 gitlink for {path}')
    git(repo, 'update-index', '--cacheinfo', f'160000,{ours[0]},{path}')
    return {'path': path, 'taken_base_pin': ours[0]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('inspect', 'check', 'sync', 'take-base-gitlink'))
    parser.add_argument('--repo', type=Path, default=Path.cwd())
    parser.add_argument('--path')
    parser.add_argument('--before', help='pre-rebase parent commit whose recorded pins may remain checked out')
    args = parser.parse_args()
    try:
        repo = repository_root(args.repo)
        before = git(repo, 'rev-parse', '--verify', '--end-of-options', f'{args.before}^{{commit}}') if args.before else None
        if args.action == 'sync':
            result = sync(repo, before)
        elif args.action == 'take-base-gitlink':
            if not args.path:
                raise ValueError('--path is required')
            result = take_base_gitlink(repo, args.path)
        else:
            result = inspect(repo, before=before)
        print(json.dumps(result, ensure_ascii=False))
        return int(args.action == 'check' and any(state['dirty'] or state['unpushed'] or state['unrecorded'] for state in result))
    except (OSError, ValueError) as error:
        print(f'submodule safety failed: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
