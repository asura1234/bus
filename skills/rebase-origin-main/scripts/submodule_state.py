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


def remote_has_commit(repo: Path, commit: str) -> bool:
    """Ask the child origin for a SHA without trusting the child's local objects."""
    remote = subprocess.run(['git', '-C', str(repo), 'remote', 'get-url', 'origin'],
                            text=True, capture_output=True)
    if remote.returncode or not remote.stdout.strip():
        return False
    # A dry-run from the child itself succeeds for objects it already owns even
    # when origin cannot serve them. An empty object store forces a remote check.
    with tempfile.TemporaryDirectory(prefix='bus-child-remote-') as directory:
        init = subprocess.run(['git', 'init', '-q', '--bare', directory], capture_output=True)
        if init.returncode:
            return False
        result = subprocess.run(
            ['git', '-C', directory, 'fetch', '--dry-run', '--no-write-fetch-head',
             remote.stdout.strip(), commit],
            text=True, capture_output=True, env={**os.environ, 'GIT_TERMINAL_PROMPT': '0'},
        )
        return result.returncode == 0


def unpublished_child_work(repo: Path, pin: str, head: str) -> str:
    # Include every local branch, even when the checkout is detached at its pin.
    # Tags and remote-tracking refs are publication evidence in the local graph.
    candidates = git(repo, 'log', '--format=%H %s', '--branches', 'HEAD',
                     '--not', '--remotes', '--tags').splitlines()
    shallow = git(repo, 'rev-parse', '--is-shallow-repository') == 'true'
    unpublished = []
    for line in candidates:
        commit = line.split(' ', 1)[0]
        # A fetched-by-SHA pin can be absent from all local advertised refs.
        # Ask its own origin; the parent branch's publication says nothing about it.
        if (commit in (pin, head) or shallow) and remote_has_commit(repo, commit):
            continue
        unpublished.append(line)
    return '\n'.join(unpublished)


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
