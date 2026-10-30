"""A deleted remote branch must not authorize discarding its local child tip."""
import json
import os
from pathlib import Path
import subprocess
import sys

from child_publication_review_test import commit, git, setup

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'


def test_deleted_remote_branch_object_is_unpublished(tmp_path):
    _, remote, parent, _, pin = setup(tmp_path)
    child = parent / 'child'
    git(child, 'switch', '-qc', 'feature')
    unpublished = commit(child, 'local-work')
    git(child, 'push', '-q', 'origin', 'feature')
    git(child, 'push', '-q', 'origin', ':feature')
    git(child, 'fetch', '-q', '--prune', 'origin')
    git(child, 'switch', '-q', '--detach', pin)
    assert git(remote, 'cat-file', '-t', unpublished) == 'commit'

    # The default v2 upload-pack may serve the dangling object even though
    # no remote ref keeps it published.
    env = {**os.environ, 'GIT_CONFIG_COUNT': '1',
           'GIT_CONFIG_KEY_0': 'protocol.version', 'GIT_CONFIG_VALUE_0': '2'}
    result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(parent)],
                            capture_output=True, text=True, env=env)
    assert result.returncode == 1, result.stdout + result.stderr
    assert unpublished in json.loads(result.stdout)[0]['unpushed']
