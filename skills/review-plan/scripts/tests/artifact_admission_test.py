"""Plan validation stays read-only; recording a round requires ignored artifact paths."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location('plan_round_artifact_admission', SCRIPTS / 'review_round.py')
round_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(round_module)


class PlanArtifactAdmissionTest(unittest.TestCase):
    def test_unignored_round_is_rejected_but_check_mode_writes_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            subprocess.run(['git', 'init', '-q', str(repo)], check=True)
            plan = repo / 'plan.md'
            original = '**状态**：create-plan-complete\n'
            plan.write_text(original)
            # Semantic plan validity is a separate contract. This test exercises the real
            # Git admission and writer boundary after that semantic preflight succeeds.
            with patch.object(round_module, 'REPO_ROOT', repo), \
                 patch.object(round_module, 'current_branch', return_value='feature'), \
                 patch.object(round_module, 'prereq_failures', return_value=[]):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    code = round_module.main(['review_round.py', str(plan), '--reviewer', 'test'])
                self.assertEqual(code, 1)
                self.assertIn('ignored', output.getvalue())
                self.assertFalse((repo / 'temp').exists())
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(round_module.main(['review_round.py', str(plan), '--check']), 0)
                self.assertFalse((repo / 'temp').exists())
                self.assertEqual(plan.read_text(), original)


if __name__ == '__main__':
    unittest.main()
