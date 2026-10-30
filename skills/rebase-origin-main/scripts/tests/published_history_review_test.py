"""R3 probes: published pin ancestors and remote-check transfer size."""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
SCRIPT = SCRIPTS / 'submodule_state.py'
sys.path.insert(0, str(SCRIPTS))
import submodule_state  # noqa: E402


def git(cwd, *args):
    return subprocess.run(['git', '-c', 'protocol.file.allow=always', *args], cwd=cwd, check=True,
                          capture_output=True, text=True).stdout.strip()


def commit(repo, name, size=1):
    (repo / name).write_bytes(os.urandom(size))
    git(repo, 'add', name)
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', name)
    return git(repo, 'rev-parse', 'HEAD')


class R3Probe(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.tmp = Path(self.temp.name)

    def test_published_pin_ancestors_off_branch_refs_are_not_unpushed(self):
        # Child origin serves B and C only through a non-branch ref (e.g. Gerrit
        # refs/changes or a review ref); the parent pins C. A normal, non-shallow
        # submodule update fetches C by SHA, which brings B with it.
        child = self.tmp / 'child-src'
        child.mkdir()
        git(child, 'init', '-q', '-b', 'main')
        commit(child, 'a')
        git(child, 'checkout', '-q', '-b', 'review')
        b = commit(child, 'b')
        c = commit(child, 'c')
        git(child, 'checkout', '-q', 'main')
        git(self.tmp, 'clone', '-q', '--bare', str(child), 'child.git')
        bare = self.tmp / 'child.git'
        git(bare, 'update-ref', 'refs/changes/01/1/1', c)
        git(bare, 'update-ref', '-d', 'refs/heads/review')
        git(bare, 'config', 'uploadpack.allowAnySHA1InWant', 'true')
        parent = self.tmp / 'parent-src'
        parent.mkdir()
        git(parent, 'init', '-q', '-b', 'main')
        git(parent, 'submodule', 'add', '-q', bare.as_uri(), 'child')
        git(parent / 'child', 'fetch', '-q', 'origin', c)
        git(parent / 'child', 'checkout', '-q', c)
        git(parent, 'add', 'child')
        git(parent, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'pin')
        git(self.tmp, 'clone', '-q', str(parent), 'work')
        git(self.tmp / 'work', 'submodule', 'update', '-q', '--init')
        work_child = self.tmp / 'work/child'
        self.assertEqual(git(work_child, 'rev-parse', 'HEAD'), c)
        self.assertEqual(git(work_child, 'rev-parse', '--is-shallow-repository'), 'false')
        self.assertTrue(submodule_state.remote_has_commit(work_child, c))
        result = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(self.tmp / 'work')],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0,
                         f'published ancestor {b} of remote-confirmed pin {c} reported: {result.stdout}{result.stderr}')


if __name__ == '__main__':
    unittest.main()
