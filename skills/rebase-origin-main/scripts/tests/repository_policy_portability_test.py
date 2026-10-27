"""Repository policy independence and clean initialized-submodule safety."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
SKILLS = SCRIPTS.parents[1]
ENV = {
    **os.environ,
    'GIT_CONFIG_COUNT': '3',
    'GIT_CONFIG_KEY_0': 'protocol.file.allow', 'GIT_CONFIG_VALUE_0': 'always',
    'GIT_CONFIG_KEY_1': 'user.name', 'GIT_CONFIG_VALUE_1': 'Probe',
    'GIT_CONFIG_KEY_2': 'user.email', 'GIT_CONFIG_VALUE_2': 'probe@example.test',
}


def git(cwd, *args):
    return subprocess.check_output(['git', '-C', str(cwd), *args], env=ENV, text=True).strip()


class RepositoryPolicyPortabilityTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()

    def published_repo(self, name='repo'):
        remote = self.root / f'{name}-remote.git'
        repo = self.root / name
        git(self.root, 'init', '-q', '--bare', '-b', 'main', str(remote))
        git(self.root, 'init', '-q', '-b', 'main', str(repo))
        (repo / 'f').write_text('a\n')
        git(repo, 'add', 'f')
        git(repo, 'commit', '-qm', 'one')
        git(repo, 'remote', 'add', 'origin', str(remote))
        git(repo, 'push', '-q', '-u', 'origin', 'main')
        return repo

    def test_policy_context_survives_unrelated_manifest_hazard(self):
        # Policy-only consumers must not inventory unrelated manifests.
        repo = self.published_repo()
        git(repo, 'fetch', '-q', 'origin')
        git(repo, 'symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/main')
        (repo / 'package.json').write_text('{"name": "x"}\n')
        (repo / 'package-lock.json').write_text('{}\n')
        (repo / 'yarn.lock').write_text('\n')
        run = subprocess.run([sys.executable, str(SCRIPTS / 'repository_context.py'), '--repo', str(repo), '--optional-base'],
                             env=ENV, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)

    def test_policy_context_without_origin_head_after_first_push(self):
        # A fresh `git init` + `git push -u` repo has no refs/remotes/origin/HEAD.
        repo = self.published_repo()
        git(repo, 'switch', '-qc', 'feature')
        run = subprocess.run([sys.executable, str(SCRIPTS / 'repository_context.py'), '--repo', str(repo), '--optional-base'],
                             env=ENV, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)

    def test_check_flags_pinned_child_commit_that_exists_only_in_worktree_modules(self):
        child_remote = self.root / 'child-remote.git'
        git(self.root, 'init', '-q', '--bare', '-b', 'main', str(child_remote))
        source = self.root / 'child-source'
        git(self.root, 'init', '-q', '-b', 'main', str(source))
        (source / 'f').write_text('a\n')
        git(source, 'add', 'f')
        git(source, 'commit', '-qm', 'one')
        git(source, 'remote', 'add', 'origin', str(child_remote))
        git(source, 'push', '-q', 'origin', 'main')
        parent = self.published_repo('parent')
        git(parent, 'submodule', 'add', '-q', str(child_remote), 'child')
        git(parent, 'commit', '-qm', 'add child')
        git(parent, 'push', '-q', 'origin', 'main')
        worktree = self.root / 'wt'
        git(parent, 'worktree', 'add', '-q', '-b', 'feat', str(worktree), 'main')
        git(worktree, 'submodule', 'update', '-q', '--init')
        (worktree / 'child/f').write_text('b\n')
        git(worktree / 'child', 'commit', '-qam', 'child commit only in this worktree')
        local_only = git(worktree / 'child', 'rev-parse', 'HEAD')
        git(worktree, 'commit', '-qam', 'pin child')
        git(worktree, 'push', '-q', 'origin', 'feat:feat')
        # Parent is clean and pushed; the pinned child commit is reachable from no child remote.
        self.assertEqual(git(worktree / 'child', 'log', '--format=%H', 'HEAD', '--not', '--remotes'), local_only)
        self.assertEqual(git(worktree, 'log', '--oneline', 'HEAD', '--not', '--remotes'), '')
        run = subprocess.run([sys.executable, str(SCRIPTS / 'submodule_state.py'), 'check', '--repo', str(worktree)],
                             env=ENV, text=True, capture_output=True)
        # Without a warning guard, force-removal of initialized submodules deletes
        # .git/worktrees/wt/modules/child and with it the only copy of local_only.
        self.assertNotEqual(run.returncode, 0, 'unpushed child commit not reported: ' + run.stdout)

    def test_review_plan_discovers_new_plan_in_new_untracked_plans_directory(self):
        repo = self.published_repo()
        git(repo, 'fetch', '-q', 'origin')
        git(repo, 'symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/main')
        git(repo, 'switch', '-qc', 'feature')
        (repo / 'docs/plans').mkdir(parents=True)
        (repo / 'docs/plans/new plan.md').write_text('# plan\n')
        script = ('import sys,json;sys.path.insert(0,sys.argv[1]);import review_round_support as r;'
                  'print(json.dumps([str(p) for p in r.detect_branch_plan_docs()]))')
        run = subprocess.run([sys.executable, '-c', script, str(SKILLS / 'review-plan/scripts')],
                             cwd=repo, env=ENV, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(run.stdout), [str(repo / 'docs/plans/new plan.md')])



if __name__ == '__main__':
    unittest.main()
