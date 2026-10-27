"""A real rebase gitlink conflict resolved to the base pin, then resynced, must leave no child work behind."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'


class SubmoduleRebaseConflictTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name).resolve()
        self.env = {**os.environ, 'GIT_ALLOW_PROTOCOL': 'file', 'GIT_AUTHOR_NAME': 'T', 'GIT_AUTHOR_EMAIL': 't@e.t',
                    'GIT_COMMITTER_NAME': 'T', 'GIT_COMMITTER_EMAIL': 't@e.t'}
        self.source, self.repo = root / 'source', root / 'parent'
        for path in (self.source, self.repo):
            path.mkdir()
            self.git(path, 'init', '-q', '-b', 'trunk')
        self.pins = []
        for text in ('one', 'two', 'three'):
            if text == 'two':
                self.git(self.source, 'switch', '-qc', 'side')
            if text == 'three':
                self.git(self.source, 'switch', '-q', 'trunk')
            (self.source / 'file').write_text(text)
            self.git(self.source, 'add', 'file')
            self.git(self.source, 'commit', '-qm', text)
            self.pins.append(self.git(self.source, 'rev-parse', 'HEAD'))
        self.git(self.repo, 'submodule', 'add', '-q', str(self.source), 'dep')
        self.child = self.repo / 'dep'
        self.pin(self.pins[0], 'base pin one')
        self.git(self.repo, 'switch', '-qc', 'feature')
        self.pin(self.pins[1], 'feature pin two')
        self.git(self.repo, 'switch', '-q', 'trunk')
        self.git(self.repo, 'submodule', 'update', '-q', '--checkout')
        self.pin(self.pins[2], 'base pin three')
        self.git(self.repo, 'switch', '-q', 'feature')
        self.git(self.repo, 'submodule', 'update', '-q', '--checkout')

    def git(self, repo, *args, check=True):
        result = subprocess.run(['git', '-C', str(repo), *args], env=self.env, text=True, capture_output=True)
        if check and result.returncode:
            raise AssertionError(result.stderr)
        return result.stdout.strip()

    def pin(self, sha, message):
        self.git(self.child, 'checkout', '-q', sha)
        self.git(self.repo, 'add', 'dep')
        self.git(self.repo, 'commit', '-qm', message)

    def tool(self, *args):
        return subprocess.run([sys.executable, str(SCRIPT), *args, '--repo', str(self.repo)], env=self.env, text=True, capture_output=True)

    def test_taking_base_gitlink_in_a_real_rebase_resolves_conflict_and_sync_lands_on_base_pin(self):
        self.assertEqual(self.tool('check').returncode, 0)
        before = self.git(self.repo, 'rev-parse', 'HEAD')
        self.git(self.repo, 'rebase', 'trunk', check=False)
        self.assertTrue(self.git(self.repo, 'ls-files', '-u', 'dep'), 'expected a gitlink conflict')
        taken = self.tool('take-base-gitlink', '--path', 'dep')
        self.assertEqual(taken.returncode, 0, taken.stderr)
        self.assertEqual(json.loads(taken.stdout)['taken_base_pin'], self.pins[2])
        self.assertEqual(self.git(self.repo, 'ls-files', '-u'), '')
        self.git(self.repo, '-c', 'core.editor=true', 'rebase', '--continue', check=False)
        self.assertFalse((self.repo / '.git/rebase-merge').exists())
        synced = self.tool('sync', '--before', before)
        self.assertEqual(synced.returncode, 0, synced.stderr)
        self.assertEqual(self.git(self.child, 'rev-parse', 'HEAD'), self.pins[2])

    def test_sync_refuses_child_commits_made_while_a_rebase_was_paused(self):
        self.git(self.repo, 'rebase', '--abort', check=False)
        (self.repo / 'notes').write_text('base\n')
        self.git(self.repo, 'switch', '-q', 'trunk')
        self.git(self.repo, 'submodule', 'update', '-q', '--checkout')
        self.git(self.repo, 'add', 'notes')
        self.git(self.repo, 'commit', '-qm', 'base notes')
        self.git(self.repo, 'switch', '-q', 'feature')
        self.git(self.repo, 'submodule', 'update', '-q', '--checkout')
        (self.repo / 'notes').write_text('feature\n')
        self.git(self.repo, 'add', 'notes')
        self.git(self.repo, 'commit', '-qm', 'feature notes')
        self.assertEqual(self.tool('check').returncode, 0)
        self.git(self.repo, 'rebase', 'trunk', check=False)
        self.assertTrue(self.git(self.repo, 'ls-files', '-u'), 'expected a paused rebase')
        before = (self.repo / '.git/rebase-merge/orig-head').read_text().strip()
        # Work committed inside the child while the parent rebase is paused.
        (self.child / 'file').write_text('child work during pause')
        self.git(self.child, 'commit', '-qam', 'unrecorded child work')
        child_work = self.git(self.child, 'rev-parse', 'HEAD')
        resumed = self.tool('check', '--before', before)
        self.assertEqual(resumed.returncode, 1, resumed.stdout + resumed.stderr)
        self.assertEqual(self.git(self.child, 'rev-parse', 'HEAD'), child_work)
        for _ in range(4):
            if not (self.repo / '.git/rebase-merge').exists():
                break
            if self.git(self.repo, 'ls-files', '-u', 'notes'):
                self.git(self.repo, 'checkout', '--theirs', '--', 'notes')
                self.git(self.repo, 'add', 'notes')
            if self.git(self.repo, 'ls-files', '-u', 'dep'):
                taken = self.tool('take-base-gitlink', '--path', 'dep')
                self.assertEqual(taken.returncode, 0, taken.stderr)
            self.git(self.repo, '-c', 'core.editor=true', 'rebase', '--continue', check=False)
        self.assertFalse((self.repo / '.git/rebase-merge').exists())
        synced = self.tool('sync', '--before', before)
        self.assertNotEqual(synced.returncode, 0, synced.stdout + synced.stderr)
        self.assertEqual(self.git(self.child, 'rev-parse', 'HEAD'), child_work,
                         'sync moved the child off its unrecorded commit')



if __name__ == '__main__':
    unittest.main()
