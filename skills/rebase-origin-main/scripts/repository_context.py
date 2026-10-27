#!/usr/bin/env python3
"""Target repository context shared by installed workflow skills."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
from typing import Sequence

from rebase_base import git


def read_policy(repo: Path) -> dict:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'gate-and-fix/scripts'))
    from repo_gate import read_policy as scan_policy
    return scan_policy(repo)


def repository_root(path: Path | None = None) -> Path:
    return Path(git(path or Path.cwd(), 'rev-parse', '--show-toplevel')).resolve()


def default_base(repo: Path, base: str | None = None) -> str:
    """Read-only lookup: fetching/refreshing belongs to the invoking workflow."""
    if base is None:
        try:
            base = git(repo, 'symbolic-ref', 'refs/remotes/origin/HEAD')
        except ValueError as error:
            raise ValueError('origin default branch unavailable; caller must fetch or pass --base') from error
        if not base.startswith('refs/remotes/origin/'):
            raise ValueError('origin/HEAD must point to an origin branch')
    git(repo, 'rev-parse', '--verify', '--end-of-options', f'{base}^{{commit}}')
    return base


def require_ignored_artifacts(repo: Path, paths: Sequence[Path]) -> None:
    """Admit fixed workflow handoff paths without editing ignore files or relocating them."""
    repo = repo.resolve()
    for path in paths:
        path = (path if path.is_absolute() else repo / path).resolve()
        if not path.is_relative_to(repo):
            raise ValueError(f'workflow artifact path is outside target repository: {path}')
        relative = path.relative_to(repo).as_posix()
        tracked = subprocess.run(['git', '-C', str(repo), 'ls-files', '-z', '--', relative], capture_output=True, check=True)
        if tracked.stdout:
            raise ValueError(f'workflow artifact path contains tracked content: {relative}')
        ignored = subprocess.run(['git', '-C', str(repo), 'check-ignore', '--quiet', '--', relative], capture_output=True)
        if ignored.returncode != 0:
            raise ValueError(f'workflow artifact path must be ignored before writes: {relative}')


def plan_template(repo: Path, bundled: Path) -> Path:
    target = repo / 'docs/templates/plan-template.md'
    return target if target.is_file() else bundled


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path.cwd())
    parser.add_argument('--base')
    parser.add_argument('--require-ignored', type=Path, action='append', default=[])
    args = parser.parse_args()
    try:
        repo = repository_root(args.repo)
        base = default_base(repo, args.base)
        require_ignored_artifacts(repo, args.require_ignored)
        print(json.dumps({'repo': str(repo), 'base': base,
                          'base_commit': git(repo, 'rev-parse', f'{base}^{{commit}}'),
                          'policy': read_policy(repo)}, ensure_ascii=False))
    except (OSError, ValueError) as error:
        print(f'repository context failed: {error}', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
