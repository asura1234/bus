"""Initialized submodules are protected before rebase and resynced to recorded pins only."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'


class SubmoduleStateTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.origin, self.repo = self.root / 'child source', self.root / 'parent'
        self.env = {**os.environ, 'GIT_ALLOW_PROTOCOL': 'file'}
        for path in (self.origin, self.repo):
            path.mkdir()
            self.git(path, 'init', '-q', '-b', 'trunk')
            self.git(path, 'config', 'user.name', 'Test')
            self.git(path, 'config', 'user.email', 'test@example.test')
        (self.origin / 'file').write_text('one\n')
        self.git(self.origin, 'add', 'file')
        self.git(self.origin, 'commit', '-qm', 'one')
        self.one = self.git(self.origin, 'rev-parse', 'HEAD')
        (self.origin / 'file').write_text('two\n')
        self.git(self.origin, 'commit', '-qam', 'two')
        self.two = self.git(self.origin, 'rev-parse', 'HEAD')
        self.git(self.repo, 'submodule', 'add', '-q', str(self.origin), 'dependency with spaces')
        self.child = self.repo / 'dependency with spaces'
        self.git(self.child, 'checkout', '-q', self.one)
        self.git(self.repo, 'add', '.gitmodules', 'dependency with spaces')
        self.git(self.repo, 'commit', '-qm', 'old pin')
        (self.repo / '.bus').mkdir()
        self.policy = self.repo / '.bus/repo.toml'
        self.policy.write_text('[submodules]\nupdate="recursive"\n')

    def git(self, repo, *args):
        return subprocess.check_output(['git', '-C', str(repo), *args], env=self.env, text=True).strip()

    def run_tool(self, action, *args):
        return subprocess.run([sys.executable, str(SCRIPT), action, '--repo', str(self.repo), *args], env=self.env, text=True, capture_output=True)

    def test_inspection_detects_dirty_untracked_and_unrecorded_child_work(self):
        self.assertTrue(SCRIPT.exists(), 'conditional submodule safety is missing')
        clean = self.run_tool('inspect')
        self.assertEqual(clean.returncode, 0, clean.stderr)
        self.assertEqual(json.loads(clean.stdout)[0]['moved'], False)
        (self.child / 'untracked').write_text('keep')
        dirty = self.run_tool('check')
        self.assertNotEqual(dirty.returncode, 0)
        self.assertIn('untracked', dirty.stdout)
        (self.child / 'untracked').unlink()
        self.git(self.child, 'checkout', '-q', self.two)
        moved = self.run_tool('check')
        self.assertEqual(moved.returncode, 1, moved.stderr)
        self.assertTrue(json.loads(moved.stdout)[0]['moved'])

    def test_sync_updates_only_to_index_pin_and_refuses_dirty_source(self):
        self.git(self.repo, 'update-index', '--cacheinfo', f'160000,{self.two},dependency with spaces')
        (self.child / 'file').write_text('uncommitted\n')
        blocked = self.run_tool('sync')
        self.assertNotEqual(blocked.returncode, 0)
        self.assertEqual((self.child / 'file').read_text(), 'uncommitted\n')
        self.git(self.child, 'restore', 'file')
        result = self.run_tool('sync')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git(self.child, 'rev-parse', 'HEAD'), self.two)
        self.assertFalse(json.loads(result.stdout)[0]['moved'])

    def test_none_policy_never_updates_and_uninitialized_paths_stay_uninitialized(self):
        self.policy.write_text('[submodules]\nupdate="none"\n')
        self.git(self.repo, 'update-index', '--cacheinfo', f'160000,{self.two},dependency with spaces')
        result = self.run_tool('sync')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git(self.child, 'rev-parse', 'HEAD'), self.one)
        self.git(self.repo, 'update-index', '--cacheinfo', f'160000,{self.one},dependency with spaces')
        self.git(self.repo, 'submodule', 'deinit', '-q', 'dependency with spaces')
        self.policy.write_text('[submodules]\nupdate="recursive"\n')
        result = self.run_tool('sync')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), [])
        self.assertFalse((self.child / '.git').exists())

    def test_non_conflict_gitlink_is_not_rewritten(self):
        before = self.git(self.repo, 'ls-files', '--stage', 'dependency with spaces')
        result = self.run_tool('take-base-gitlink', '--path', 'dependency with spaces')
        self.assertEqual(result.returncode, 2)
        self.assertIn('no unique stage-2 gitlink', result.stderr)
        self.assertEqual(self.git(self.repo, 'ls-files', '--stage', 'dependency with spaces'), before)

    def test_nested_child_work_is_visible_even_when_parent_ignores_submodule_status(self):
        inner = self.root / 'nested source'
        inner.mkdir()
        self.git(inner, 'init', '-q', '-b', 'trunk')
        (inner / 'file').write_text('nested base\n')
        self.git(inner, 'add', 'file')
        self.git(inner, '-c', 'user.name=Test', '-c', 'user.email=test@example.test', 'commit', '-qm', 'nested')
        self.git(self.child, 'submodule', 'add', '-q', str(inner), 'nested dependency')
        self.git(self.child, '-c', 'user.name=Test', '-c', 'user.email=test@example.test', 'commit', '-qam', 'nested dependency')
        self.git(self.repo, 'add', 'dependency with spaces')
        self.git(self.repo, 'commit', '-qm', 'record nested dependency')
        (self.child / 'nested dependency/file').write_text('retain nested work\n')
        result = self.run_tool('check')
        self.assertEqual(result.returncode, 1, result.stderr)
        states = json.loads(result.stdout)
        self.assertEqual(states[0]['dirty'], '')
        self.assertIn('file', states[1]['dirty'])
        self.assertEqual(states[1]['path'], 'dependency with spaces/nested dependency')

    def test_take_base_gitlink_records_stage_two_not_checked_out_child(self):
        self.git(self.repo, 'update-index', '--force-remove', 'dependency with spaces')
        record = '\n'.join(f'160000 {sha} {stage}\tdependency with spaces' for stage, sha in [(1, self.one), (2, self.two), (3, self.one)])+'\n'
        subprocess.run(['git', '-C', str(self.repo), 'update-index', '--index-info'], input=record, env=self.env, text=True, check=True)
        result = self.run_tool('take-base-gitlink', '--path', 'dependency with spaces')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(self.two, self.git(self.repo, 'ls-files', '--stage', 'dependency with spaces'))
        self.assertEqual(self.git(self.child, 'rev-parse', 'HEAD'), self.one)


if __name__ == '__main__':
    unittest.main()
