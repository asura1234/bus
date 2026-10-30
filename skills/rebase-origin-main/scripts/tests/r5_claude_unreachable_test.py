"""R5 probe: on a server that allows any SHA in want, protocol v0 still serves dangling objects."""
import json
import subprocess
import sys
from pathlib import Path

import pytest

from child_publication_review_test import commit, git, setup

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'


@pytest.mark.parametrize('any_sha', [False, True])
def test_recorded_pin_from_deleted_remote_branch_is_unpublished(tmp_path, any_sha):
    _, remote, parent, _, _ = setup(tmp_path)
    if any_sha:
        git(remote, 'config', 'uploadpack.allowAnySHA1InWant', 'true')
    child = parent / 'child'
    git(child, 'switch', '-qc', 'feature')
    unpublished = commit(child, 'local-work')
    git(child, 'push', '-q', 'origin', 'feature')
    # Record the child commit in the parent, then the child branch is deleted (e.g. squash-merged).
    git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qam', 'bump child')
    git(child, 'push', '-q', 'origin', ':feature')
    git(child, 'fetch', '-q', '--prune', 'origin')
    assert git(remote, 'for-each-ref', '--contains', unpublished) == ''
    assert git(remote, 'cat-file', '-t', unpublished) == 'commit'
    result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(parent)],
                            capture_output=True, text=True)
    print(any_sha, result.returncode, result.stdout)
    assert result.returncode == 1, result.stdout + result.stderr
    assert unpublished in json.loads(result.stdout)[0]['unpushed']
