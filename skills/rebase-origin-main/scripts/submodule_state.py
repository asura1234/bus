#!/usr/bin/env python3
"""Inspect initialized submodule work and resync only recorded gitlink commits."""
from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys

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


def inspect(repo: Path, prefix: str = '') -> list[dict]:
    states = []
    for path in paths(repo):
        full = repo / path
        if repository_root(full) != full.resolve():
            raise ValueError(f'submodule Git root mismatch: {path}')
        record = git(repo, 'ls-files', '--stage', '--', path).split('\t', 1)[0].split()
        if len(record) != 3 or record[0] != '160000' or record[2] != '0':
            raise ValueError(f'unresolved or missing gitlink: {path}')
        head = git(full, 'rev-parse', 'HEAD')
        states.append({'path': prefix + path, 'pin': record[1], 'head': head,
                       'moved': head != record[1],
                       'dirty': git(full, 'status', '--porcelain', '--untracked-files=all', '--ignore-submodules=all')})
        states.extend(inspect(full, prefix + path + '/'))
    return states


def sync(repo: Path) -> list[dict]:
    policy = read_policy(repo)['submodule_update']
    if policy not in ('recursive', 'none'):
        raise ValueError('unsupported submodule_update policy')
    before = inspect(repo)
    if any(state['dirty'] for state in before):
        raise ValueError('submodule has uncommitted work; refusing resync: ' + json.dumps(before))
    initialized = paths(repo)
    if policy == 'recursive' and initialized:
        git(repo, 'submodule', 'update', '--checkout', '--recursive', '--', *initialized)
    after = inspect(repo)
    if any(state['dirty'] or state['moved'] for state in after):
        raise ValueError('submodules still differ from recorded gitlinks; no push: ' + json.dumps(after))
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
    args = parser.parse_args()
    try:
        repo = repository_root(args.repo)
        if args.action == 'sync':
            result = sync(repo)
        elif args.action == 'take-base-gitlink':
            if not args.path:
                raise ValueError('--path is required')
            result = take_base_gitlink(repo, args.path)
        else:
            result = inspect(repo)
        print(json.dumps(result, ensure_ascii=False))
        return int(args.action == 'check' and any(state['dirty'] or state['moved'] for state in result))
    except (OSError, ValueError) as error:
        print(f'submodule safety failed: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
