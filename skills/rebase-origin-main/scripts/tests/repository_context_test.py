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
sys.path.insert(0, str(SCRIPTS))


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
            admission = [sys.executable, str(CONTEXT), '--base', 'HEAD', '--require-ignored', 'temp/review-pr/feature']
            unignored = subprocess.run(admission, cwd=nested, text=True, capture_output=True)
            self.assertEqual(unignored.returncode, 2)
            self.assertIn('must be ignored before writes', unignored.stderr)
            (repo / '.git/info/exclude').write_text('temp/\n')
            admitted = subprocess.run(admission, cwd=nested, text=True, capture_output=True)
            self.assertEqual(admitted.returncode, 0, admitted.stderr)
            self.assertFalse((repo / 'temp').exists())
            git('symbolic-ref', '--delete', 'refs/remotes/origin/HEAD')
            missing = subprocess.run([sys.executable, str(CONTEXT)], cwd=nested, text=True, capture_output=True)
            self.assertEqual(missing.returncode, 2)
            self.assertIn('--base', missing.stderr)

    def test_fixed_artifact_paths_require_ignore_and_reject_tracked_content(self):
        self.assertTrue(CONTEXT.exists())
        spec = importlib.util.spec_from_file_location('repository_context', CONTEXT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.assertTrue(hasattr(module, 'require_ignored_artifacts'), 'artifact admission helper is missing')
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            subprocess.run(['git', 'init', '-q', str(repo)], check=True)
            root = repo / 'temp/review-plan/feature'
            with self.assertRaisesRegex(ValueError, 'ignored'):
                module.require_ignored_artifacts(repo, [root])
            self.assertFalse(root.exists())
            (repo / '.git/info/exclude').write_text('temp/\n')
            module.require_ignored_artifacts(repo, [root, root / '.last-plan'])
            self.assertFalse(root.exists())
            root.mkdir(parents=True)
            tracked = root / 'tracked.md'
            tracked.write_text('retain tracked data')
            subprocess.run(['git', '-C', str(repo), 'add', '--force', str(tracked)], check=True)
            with self.assertRaisesRegex(ValueError, 'tracked'):
                module.require_ignored_artifacts(repo, [root])
            self.assertEqual(tracked.read_text(), 'retain tracked data')
            with self.assertRaisesRegex(ValueError, 'outside'):
                module.require_ignored_artifacts(repo, [repo.parent / 'outside'])

    def test_optional_base_admits_policy_in_a_local_only_repo_but_rejects_bad_explicit_base(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            subprocess.run(['git', 'init', '-q', '-b', 'trunk', str(repo)], check=True)
            subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Test', '-c', 'user.email=test@example.test', 'commit', '--allow-empty', '-qm', 'base'], check=True)
            result = subprocess.run([sys.executable, str(CONTEXT), '--optional-base'], cwd=repo, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads(result.stdout)
            self.assertIsNone(data['base'])
            self.assertIsNone(data['base_commit'])
            self.assertIn('commit_style', data['policy'])
            invalid = subprocess.run([sys.executable, str(CONTEXT), '--optional-base', '--base', 'missing'], cwd=repo, text=True, capture_output=True)
            self.assertEqual(invalid.returncode, 2, invalid.stderr)

    def test_branch_guard_needs_no_base_or_first_commit_and_still_protects_main(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            subprocess.run(['git', 'init', '-q', '-b', 'feature', str(repo)], check=True)
            args = [sys.executable, str(CONTEXT), '--optional-base', '--check-branch']
            allowed = subprocess.run([*args, 'feature'], cwd=repo, text=True, capture_output=True)
            self.assertEqual(allowed.returncode, 0, allowed.stderr)
            self.assertIsNone(json.loads(allowed.stdout)['base'])
            protected = subprocess.run([*args, 'main'], cwd=repo, text=True, capture_output=True)
            self.assertEqual(protected.returncode, 2)
            self.assertIn('protected branch', protected.stderr)

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
