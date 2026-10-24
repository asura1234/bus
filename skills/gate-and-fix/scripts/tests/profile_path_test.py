import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402


class ProfilePathTest(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "POSIX symlink path aliases")
    def test_unit_evidence_accepts_a_symlinked_target_directory(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            actual = root / "actual-target"
            actual.mkdir()
            alias = root / "target"
            alias.symlink_to(actual, target_is_directory=True)
            output = root / "reports"

            def collect(*args, **kwargs):
                (actual / f"{root.name}-unit-1.profraw").write_bytes(b"unit evidence")
                return 0

            with (
                patch.dict(os.environ, {"CARGO_LLVM_COV_TARGET_DIR": str(alias)}, clear=True),
                patch.object(quality, "ROOT", root),
                patch.object(quality, "OUTPUT", output),
                patch.object(quality, "head", return_value="fixture"),
                patch.object(quality, "rust_test", side_effect=collect),
                patch.object(quality, "run", return_value=0),
                patch.object(quality, "python_files", return_value=[root / "case_test.py"]),
            ):
                self.assertEqual(quality.unit(), 0)
                quality.require_unit_profraw(quality.state())
