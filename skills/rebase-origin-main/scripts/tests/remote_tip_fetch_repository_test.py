"""A publication check must not rewrite the child and must accept a review-ref ancestor."""
import json
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'


def git(cwd, *args):
    return subprocess.run(['git', '-c', 'protocol.file.allow=always', *args], cwd=cwd, check=True,
                          capture_output=True, text=True).stdout.strip()


def commit(repo, name):
    (repo / name).write_text(name)
    git(repo, 'add', name)
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', name)
    return git(repo, 'rev-parse', 'HEAD')


def review_remote(tmp_path):
    source = tmp_path / 'child-src'
    source.mkdir()
    git(source, 'init', '-q', '-b', 'main')
    commit(source, 'a')
    git(tmp_path, 'clone', '-q', '--bare', str(source), 'child.git')
    return source, tmp_path / 'child.git'


def publish_review(source, bare):
    git(source, 'checkout', '-q', '-b', 'review')
    pin = commit(source, 'b')
    tip = commit(source, 'c')
    git(bare, 'fetch', '-q', str(source), '+refs/heads/review:refs/changes/01/1/1')
    return pin, tip


def add_child(tmp_path, bare, url):
    parent = tmp_path / 'parent-src'
    parent.mkdir()
    git(parent, 'init', '-q', '-b', 'main')
    git(parent, 'submodule', 'add', '-q', url, 'child')
    return parent, parent / 'child'


def test_review_ref_ancestor_fetch_does_not_partial_clone_the_child(tmp_path):
    source, bare = review_remote(tmp_path)
    git(bare, 'config', 'uploadpack.allowFilter', 'true')
    pin, tip = publish_review(source, bare)
    parent, child = add_child(tmp_path, bare, bare.as_uri())
    git(child, 'fetch', '-q', 'origin', pin)
    git(child, 'checkout', '-q', pin)
    git(parent, 'add', 'child')
    git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'pin')
    assert subprocess.run(['git', '-C', str(child), 'cat-file', '-e', tip],
                          capture_output=True).returncode != 0
    before = git(child, 'config', '--get', 'core.repositoryformatversion')
    result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(parent)],
                            capture_output=True, text=True)
    after = git(child, 'config', '--get', 'core.repositoryformatversion')
    promisor = subprocess.run(['git', '-C', str(child), 'config', '--local', '--list'],
                              capture_output=True, text=True).stdout
    assert result.returncode == 0, result.stdout + result.stderr
    assert pin not in json.loads(result.stdout)[0]['unpushed']
    assert before == after == '0', promisor
    assert 'promisor=true' not in promisor


def test_path_remote_confirms_review_ref_ancestor_whose_tip_is_absent(tmp_path):
    source, bare = review_remote(tmp_path)
    parent, child = add_child(tmp_path, bare, str(bare))
    pin, tip = publish_review(source, bare)
    assert subprocess.run(['git', '-C', str(child), 'cat-file', '-e', tip],
                          capture_output=True).returncode != 0
    git(child, 'fetch', '-q', 'origin', pin)
    git(child, 'checkout', '-q', pin)
    git(parent, 'add', 'child')
    git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'pin')
    before = git(child, 'config', '--get', 'core.repositoryformatversion')
    result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(parent)],
                            capture_output=True, text=True)
    after = git(child, 'config', '--get', 'core.repositoryformatversion')
    promisor = subprocess.run(['git', '-C', str(child), 'config', '--local', '--list'],
                              capture_output=True, text=True).stdout
    states = json.loads(result.stdout)
    assert result.returncode == 0 and pin not in states[0]['unpushed'], result.stdout + result.stderr
    assert before == after == '0' and 'promisor=true' not in promisor, promisor
