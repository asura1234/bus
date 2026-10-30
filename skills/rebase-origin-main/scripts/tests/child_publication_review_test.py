"""Exercise child publication checks for local tags and SHA-fetched history."""
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'

def git(repo, *args):
    return subprocess.run(['git', '-c', 'protocol.file.allow=always', '-C', str(repo), *args], check=True, capture_output=True, text=True).stdout.strip()

def commit(repo, name):
    (repo / name).write_text(name)
    git(repo, 'add', name)
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', name)
    return git(repo, 'rev-parse', 'HEAD')

def setup(root):
    source = root / 'source'
    source.mkdir()
    git(source, 'init', '-q', '-b', 'main')
    a = commit(source, 'a')
    b = commit(source, 'b')
    remote = root / 'remote.git'
    git(root, 'clone', '-q', '--bare', str(source), str(remote))
    parent = root / 'parent'
    parent.mkdir()
    git(parent, 'init', '-q', '-b', 'main')
    git(parent, 'submodule', 'add', '-q', str(remote), 'child')
    git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qam', 'pin')
    return source, remote, parent, a, b

def check(parent):
    return subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(parent)], capture_output=True, text=True)

def test_local_tag_does_not_hide_unpublished_child_branch():
    with tempfile.TemporaryDirectory() as tmp:
        source, remote, parent, a, b = setup(Path(tmp))
        child = parent / 'child'
        git(child, 'switch', '-qc', 'local-only')
        unique = commit(child, 'local')
        git(child, 'tag', 'local-only-tag')
        git(child, 'switch', '-q', '--detach', b)
        result = check(parent)
        assert result.returncode == 1 and unique in result.stdout, result.stdout + result.stderr

def test_published_pin_without_local_refs_includes_published_ancestors():
    with tempfile.TemporaryDirectory() as tmp:
        source, remote, parent, a, b = setup(Path(tmp))
        child = parent / 'child'
        # The remote moves its branch to an unrelated history but retains the
        # old pin under a tag. Fetch that pin by SHA without fetching the tag.
        tree = git(source, 'rev-parse', 'HEAD^{tree}')
        orphan = git(source, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit-tree', '-m', 'orphan', tree)
        git(source, 'tag', 'published-old', b)
        git(source, 'update-ref', 'refs/heads/main', orphan)
        git(remote, 'fetch', '-q', str(source), '+refs/heads/main:refs/heads/main', 'refs/tags/published-old:refs/tags/published-old')
        git(child, 'fetch', '-q', '--no-tags', 'origin', '+refs/heads/main:refs/remotes/origin/main')
        result = check(parent)
        assert result.returncode == 0, result.stdout + result.stderr
