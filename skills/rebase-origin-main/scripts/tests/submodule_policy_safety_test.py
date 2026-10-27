"""Behavioral probes for initialized-submodule worktree cleanup safety."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'submodule_state.py'


class SubmodulePolicySafetyTest(unittest.TestCase):
    def git(self, repo, *args, check=True):
        result = subprocess.run(['git', '-C', str(repo), *args], env=self.env,
                                text=True, capture_output=True, check=check)
        return result.stdout.strip() if check else result

    def test_unpushed_pinned_child_blocks_automatic_force_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.env = {**os.environ, 'GIT_ALLOW_PROTOCOL': 'file',
                        'GIT_AUTHOR_NAME': 'Test', 'GIT_AUTHOR_EMAIL': 'test@example.test',
                        'GIT_COMMITTER_NAME': 'Test', 'GIT_COMMITTER_EMAIL': 'test@example.test'}
            child_source, main, worktree = root / 'child-source', root / 'main', root / 'feature'
            for repo in (child_source, main):
                repo.mkdir()
                self.git(repo, 'init', '-q', '-b', 'trunk')
                (repo / 'file').write_text('base\n')
                self.git(repo, 'add', 'file')
                self.git(repo, 'commit', '-qm', 'base')
            self.git(main, 'worktree', 'add', '-q', '-b', 'feature', str(worktree))
            self.git(worktree, 'submodule', 'add', '-q', str(child_source), 'dependency')
            child = worktree / 'dependency'
            (child / 'file').write_text('unique child implementation\n')
            self.git(child, 'commit', '-qam', 'child implementation')
            child_head = self.git(child, 'rev-parse', 'HEAD')
            child_gitdir = Path(self.git(child, 'rev-parse', '--absolute-git-dir'))
            self.git(worktree, 'add', '.gitmodules', 'dependency')
            self.git(worktree, 'commit', '-qm', 'record child implementation')
            # The parent is published; the child's source has not received its commit.
            self.git(main, 'update-ref', 'refs/remotes/origin/feature', self.git(worktree, 'rev-parse', 'HEAD'))
            self.assertNotEqual(self.git(child_source, 'cat-file', '-e', child_head, check=False).returncode, 0)
            inspect = subprocess.run([sys.executable, str(SCRIPT), 'inspect', '--repo', str(worktree)],
                                     env=self.env, text=True, capture_output=True, check=True)
            states = json.loads(inspect.stdout)
            self.assertEqual(self.git(worktree, 'status', '--short'), '')
            self.assertEqual(self.git(worktree, 'log', '--oneline', 'HEAD', '--not', '--remotes'), '')
            self.assertTrue(states)
            self.assertFalse(any(state['dirty'] or state['moved'] for state in states), states)
            admission = subprocess.run([sys.executable, str(SCRIPT), 'check', '--repo', str(worktree)],
                                       env=self.env, text=True, capture_output=True)
            # Only an all-clear guard permits automatic force removal. Explicit loss
            # authorization is a separate user decision, never manufactured by this fixture.
            if admission.returncode == 0:
                self.git(main, 'worktree', 'remove', '--force', str(worktree))
            self.assertTrue(child_gitdir.exists(),
                            'automatic clean-child removal deleted the sole object store for the recorded child commit')
            self.assertEqual(admission.returncode, 1, admission.stdout + admission.stderr)
            self.assertIn(child_head, json.loads(admission.stdout)[0]['unpushed'])
            self.git(child, 'cat-file', '-e', child_head)



if __name__ == '__main__':
    unittest.main()
