"""The prologue entry points of both review skills must actually start.

When the shared lane / round / ledger part was consolidated into `cli_extensions/review_round_common.py`,
`skills/review-plan/scripts/review_round.py` was missed — it still imported those symbols from
`review_round_support`, so `/review-plan` died at import time. The whole Python test suite was green at
the time: no test case had ever launched this entry point.

So what is pinned here is not some function's behavior, but the most basic fact that "these scripts can
be launched". An import error surfaces nowhere else — no unit test imports a skill's entry script.
"""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]

PROLOGUES = [
    "skills/review-pr/scripts/review_round.py",
    "skills/review-plan/scripts/review_round.py",
    "skills/review-plan/scripts/review_round_support.py",
]


class PrologueEntrypointTests(unittest.TestCase):
    def test_prologue_script_imports_cleanly(self) -> None:
        """Run the scripts directly in the form the SKILL documents: the only allowed error is argparse's usage hint, never an import failure.

        This must use `python3 <path>`, not runpy — only the former puts the script's directory into
        sys.path[0], which is exactly how these scripts find their sibling modules. Testing with runpy
        would test a different invocation.
        """
        for script in PROLOGUES:
            with self.subTest(script=script):
                result = subprocess.run(
                    [sys.executable, str(REPO_ROOT / script)],
                    cwd=REPO_ROOT,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                combined = result.stdout + result.stderr
                self.assertNotIn("ImportError", combined, f"{script} import failed: {combined}")
                self.assertNotIn("ModuleNotFoundError", combined, f"{script} missing dependency: {combined}")
                self.assertNotIn("Traceback", combined, f"{script} crashed on startup: {combined}")


if __name__ == "__main__":
    unittest.main()
