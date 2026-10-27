"""Read-only review prologues consume the target's default branch and gate policy."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SKILLS = Path(__file__).resolve().parents[3]


class GenericReviewTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name).resolve()
        self.git('init', '-q', '-b', 'trunk')
        self.git('config', 'user.name', 'Review Test')
        self.git('config', 'user.email', 'review@example.test')
        (self.repo / '.gitignore').write_text('temp/\n')
        (self.repo / 'file').write_text('one\n')
        self.git('add', '.gitignore', 'file')
        self.git('commit', '-qm', 'base')
        base = self.git('rev-parse', 'HEAD')
        self.git('update-ref', 'refs/remotes/origin/trunk', base)
        self.git('symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/trunk')
        self.git('checkout', '-qb', 'feature')
        (self.repo / 'file').write_text('two\n')
        (self.repo / 'docs/plans').mkdir(parents=True)
        (self.repo / 'docs/plans/work.md').write_text('# Target plan\n')
        self.git('add', 'file', 'docs/plans/work.md')
        self.git('commit', '-qm', 'feature')
        self.locks = self.repo / 'temp/review-pr/feature'
        self.locks.mkdir(parents=True)
        (self.locks / '.locked-goal').write_text('Deliver the requested behavior\n')
        (self.locks / '.locked-non-goals').write_text('无\n')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True).strip()

    def test_review_pr_defaults_to_target_trunk_and_preserves_goal_locks(self):
        run = subprocess.run([sys.executable, str(SKILLS / 'review-pr/scripts/review_round.py'), '--reviewer', 'generic'], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertIn('BASE=refs/remotes/origin/trunk', run.stdout)
        self.assertEqual((self.locks / '.locked-goal').read_text(), 'Deliver the requested behavior\n')
        self.assertEqual(self.git('status', '--porcelain'), '')

    def test_review_pr_refuses_unignored_artifact_writes_and_preserves_locks(self):
        (self.repo / '.gitignore').write_text('')
        self.git('commit', '-qam', 'no temp ignore contract')
        run = subprocess.run([sys.executable, str(SKILLS / 'review-pr/scripts/review_round.py'), '--reviewer', 'unignored'], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn('ignored', run.stdout)
        self.assertFalse((self.locks / 'unignored').exists())
        self.assertEqual((self.locks / '.locked-goal').read_text(), 'Deliver the requested behavior\n')

    def test_review_plan_discovery_preserves_spaces_and_dirty_paths(self):
        self.git('mv', 'docs/plans/work.md', 'docs/plans/space plan.md')
        self.git('commit', '-qm', 'plan name with spaces')
        (self.repo / 'docs/plans/dirty plan.md').write_text('# Dirty plan\n')
        script = "import sys,json;sys.path.insert(0,sys.argv[1]);import review_round_support as r;print(json.dumps([str(p) for p in r.detect_branch_plan_docs()]))"
        run = subprocess.run([sys.executable, '-c', script, str(SKILLS / 'review-plan/scripts')], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(run.stdout), [str(self.repo / 'docs/plans/dirty plan.md'), str(self.repo / 'docs/plans/space plan.md')])

    def test_review_plan_discovers_target_plan_without_origin_master(self):
        script = "import sys,json;sys.path.insert(0,sys.argv[1]);import review_round_support as r;print(json.dumps([str(p) for p in r.detect_branch_plan_docs()]))"
        run = subprocess.run([sys.executable, '-c', script, str(SKILLS / 'review-plan/scripts')], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(run.stdout), [str(self.repo / 'docs/plans/work.md')])

    def test_standalone_template_check_uses_the_target_template(self):
        target = self.repo / 'docs/templates/plan-template.md'
        target.parent.mkdir(parents=True)
        target.write_text('## Target-specific section\n')
        plan = self.repo / 'target-plan.md'
        plan.write_text('## Target-specific section\n\nVerified target content.\n')
        run = subprocess.run([sys.executable, str(SKILLS / 'review-plan/scripts/plan_template_check.py'), str(plan)], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_review_plan_accepts_target_gate_instead_of_foreign_bus_commands(self):
        (self.repo / '.bus').mkdir()
        (self.repo / '.bus/repo.toml').write_text('gate_command="printf target-gate"\n')
        plan = '**创建日期**：2026-10-27\n\n## 测试计划\n\n### 全局 EXIT CHECK\n\n1. `printf target-gate`\n'
        script = "import sys,json;sys.path.insert(0,sys.argv[1]);from final_gate_contract import check_final_gate_contract;print(json.dumps(check_final_gate_contract(sys.argv[2])))"
        run = subprocess.run([sys.executable, '-c', script, str(SKILLS / 'review-plan/scripts'), plan], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(run.stdout), [])
        wrong = subprocess.run([sys.executable, '-c', script, str(SKILLS / 'review-plan/scripts'), plan.replace('printf target-gate', 'just lint')], cwd=self.repo, text=True, capture_output=True)
        self.assertTrue(json.loads(wrong.stdout))


if __name__ == '__main__':
    unittest.main()
