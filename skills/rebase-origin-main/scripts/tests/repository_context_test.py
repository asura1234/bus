"""Generic policy/default/template lookup must use the target repo, not the skill home."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
CONTEXT = SCRIPTS / 'repository_context.py'


class RepositoryContextTest(unittest.TestCase):
    def test_nested_repo_context_reports_profile_and_full_default_ref(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
            git('init', '-q', '-b', 'trunk')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.test', 'commit', '--allow-empty', '-qm', 'base')
            git('update-ref', 'refs/remotes/origin/trunk', git('rev-parse', 'HEAD'))
            git('symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/trunk')
            git('branch', 'origin/trunk')
            (repo / '.bus').mkdir()
            (repo / '.bus/repo.toml').write_text('gate_command="printf fake"\ncommit_style="custom repository style"\n[submodules]\nupdate="none"\n')
            nested = repo / 'nested'
            nested.mkdir()
            run = subprocess.run([sys.executable, str(CONTEXT)], cwd=nested, text=True, capture_output=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            data = json.loads(run.stdout)
            self.assertEqual(data['repo'], str(repo))
            self.assertEqual(data['base'], 'refs/remotes/origin/trunk')
            self.assertEqual(data['policy']['commit_style'], 'custom repository style')
            self.assertEqual(data['policy']['submodule_update'], 'none')
            self.assertFalse((repo / 'temp').exists())
            explicit = subprocess.run([sys.executable, str(CONTEXT), '--base', 'HEAD'], cwd=nested, text=True, capture_output=True)
            self.assertEqual(explicit.returncode, 0, explicit.stderr)
            self.assertEqual(json.loads(explicit.stdout)['base'], 'HEAD')
            git('symbolic-ref', '--delete', 'refs/remotes/origin/HEAD')
            missing = subprocess.run([sys.executable, str(CONTEXT)], cwd=nested, text=True, capture_output=True)
            self.assertEqual(missing.returncode, 2)
            self.assertIn('--base', missing.stderr)

    def test_review_template_uses_target_override_with_bundled_fallback(self):
        self.assertTrue(CONTEXT.exists(), 'repository policy accessor is missing')
        spec = importlib.util.spec_from_file_location('repository_context', CONTEXT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            bundled = repo / 'bundled.md'
            bundled.write_text('bundled')
            self.assertEqual(module.plan_template(repo, bundled), bundled)
            target = repo / 'docs/templates/plan-template.md'
            target.parent.mkdir(parents=True)
            target.write_text('target')
            self.assertEqual(module.plan_template(repo, bundled), target)


if __name__ == '__main__':
    unittest.main()
