import json
import subprocess
import sys
from pathlib import Path
from child_publication_review_test import git, commit, setup

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'

def test_deleted_recorded_pin_on_allow_any_origin_is_unpublished(tmp_path):
    _, remote, parent, _, _ = setup(tmp_path)
    child = parent / 'child'
    git(child, 'switch', '-qc', 'feature')
    lost = commit(child, 'only-copy')
    git(child, 'push', '-q', 'origin', 'feature')
    git(parent, 'add', 'child')
    git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'record pin')
    git(remote, 'config', 'uploadpack.allowAnySHA1InWant', 'true')
    git(child, 'push', '-q', 'origin', ':feature')
    git(child, 'fetch', '-q', '--prune', 'origin')
    result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(parent)], capture_output=True, text=True)
    states = json.loads(result.stdout)
    assert result.returncode == 1 and lost in states[0]['unpushed'], result.stdout + result.stderr
