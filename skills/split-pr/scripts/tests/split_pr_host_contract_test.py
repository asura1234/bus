"""The split-pr instructions preserve target defaults and the forge integration boundary."""
from pathlib import Path
import unittest


class SplitPrHostContractTest(unittest.TestCase):
    def test_default_base_and_publication_use_the_repository_and_forge_layers(self):
        text = (Path(__file__).resolve().parents[2] / 'SKILL.md').read_text()
        self.assertIn('repository_context.py', text)
        self.assertIn('refs/remotes/', text)
        self.assertNotIn('base_ref = --base or origin/master', text)
        self.assertNotIn('gh pr ', text)
        self.assertIn('publish pending forge B adapter', text)
        self.assertIn('remains on its recorded integration base', text)


if __name__ == '__main__':
    unittest.main()
