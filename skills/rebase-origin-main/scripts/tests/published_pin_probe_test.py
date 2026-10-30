"""Published submodule pins must not be mistaken for unpublished child work."""
import subprocess
import sys
import tempfile
import unittest
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


class PublishedPinFalsePositive(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.tmp = Path(self.temp.name)
        child = self.tmp / 'child-src'
        child.mkdir()
        git(child, 'init', '-q', '-b', 'main')
        self.a = commit(child, 'a')
        git(child, 'tag', 'v1')
        commit(child, 'b')
        git(self.tmp, 'clone', '-q', '--bare', str(child), 'child.git')
        git(self.tmp / 'child.git', 'config', 'uploadpack.allowAnySHA1InWant', 'true')
        parent = self.tmp / 'parent-src'
        parent.mkdir()
        git(parent, 'init', '-q', '-b', 'main')
        git(parent, 'submodule', 'add', '-q', (self.tmp / 'child.git').as_uri(), 'child')
        git(parent / 'child', 'checkout', '-q', self.a)
        git(parent, 'add', 'child')
        git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'pin')
        git(self.tmp, 'clone', '-q', '--bare', str(parent), 'parent.git')

    def check(self, clone):
        result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(clone)],
                                capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr

    def test_shallow_submodule_pinned_to_published_non_tip_commit_is_not_unpushed(self):
        git(self.tmp / 'child.git', 'tag', '-d', 'v1')
        git(self.tmp, 'clone', '-q', (self.tmp / 'parent.git').as_uri(), 'work')
        git(self.tmp / 'work', 'submodule', 'update', '-q', '--init', '--depth', '1')
        self.assertEqual(git(self.tmp / 'work/child', 'rev-parse', '--is-shallow-repository'), 'true')
        self.assertEqual(git(self.tmp / 'work/child', 'tag', '--list'), '')
        code, out = self.check(self.tmp / 'work')
        self.assertEqual(code, 0, f'published pin {self.a} reported as child work: {out}')

    def test_submodule_pinned_to_published_tag_commit_is_not_unpushed(self):
        git(self.tmp / 'child.git', 'update-ref', 'refs/heads/main', self.a)
        # Move the published branch past the tag via a branch that does not contain it.
        git(self.tmp / 'child.git', 'update-ref', 'refs/heads/main',
            git(self.tmp / 'child.git', '-c', 'user.name=t', '-c', 'user.email=t@t',
                'commit-tree', '-m', 'rewritten', git(self.tmp / 'child.git', 'rev-parse', 'v1^{tree}')))
        git(self.tmp, 'clone', '-q', (self.tmp / 'parent.git').as_uri(), 'work2')
        git(self.tmp / 'work2', 'submodule', 'update', '-q', '--init')
        git(self.tmp / 'work2/child', 'fetch', '-q', 'origin', 'tag', 'v1')
        code, out = self.check(self.tmp / 'work2')
        self.assertEqual(code, 0, f'tag-published pin {self.a} reported as child work: {out}')


if __name__ == '__main__':
    unittest.main()
