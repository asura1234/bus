import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from coverage_run import MARKER, MERGED, Run  # noqa: E402
from profile_report import merged_profile  # noqa: E402


class ProfileReportTest(unittest.TestCase):
    def test_report_lookup_is_current_run_only_and_removed_even_on_export_failure(self):
        for failure in (False, True):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as folder:
                root = Path(folder).resolve()
                with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target"),
                                             "LLVM_PROFDATA": "fixture-profdata"}, clear=True):
                    with Run(root, root / "reports", "gate") as run:
                        raw = run.path / "unit.profraw"
                        raw.write_bytes(b"current counters")

                        def merge(*argv):
                            self.assertEqual(Path(argv[argv.index("-f") + 1]).read_text(), str(raw) + "\n")
                            Path(argv[argv.index("-o") + 1]).write_bytes(raw.read_bytes())
                            return 0

                        index = run.base / (root.name + ".profdata")
                        try:
                            with merged_profile(root, run.base, dict(os.environ), merge):
                                self.assertEqual(index.read_bytes(), b"current counters")
                                self.assertTrue(index.samefile(run.path / MERGED))
                                if failure:
                                    raise ValueError("export failed")
                        except ValueError:
                            self.assertTrue(failure)
                        self.assertFalse(index.exists())
                    self.assertFalse(run.path.exists())

    def test_stale_flat_data_cannot_enter_a_run_report(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=True):
                with Run(root, root / "reports", "gate") as run:
                    (run.path / "unit.profraw").write_bytes(b"current")
                    stale = run.base / "another.profraw"
                    stale.write_bytes(b"stale")
                    with self.assertRaisesRegex(ValueError, "unexpected flat"):
                        with merged_profile(root, run.base, dict(os.environ), lambda *args: 0):
                            self.fail("stale input was accepted")
                    self.assertEqual(stale.read_bytes(), b"stale")

    def test_dead_run_reaping_removes_only_its_own_report_lookup(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=True):
                abandoned = Run(root, root / "reports", "gate")
                abandoned.__enter__()
                merged = abandoned.path / MERGED
                merged.write_bytes(b"owned index")
                index = abandoned.base / (root.name + ".profdata")
                os.link(merged, index)
                marker = abandoned.path / MARKER
                data = json.loads(marker.read_text())
                data["start"] = "dead owner incarnation"
                marker.write_text(json.dumps(data))
                os.environ.clear()
                os.environ.update(abandoned.original)
                with Run(root, root / "reports", "gate"):
                    self.assertFalse(index.exists())
                    self.assertFalse(abandoned.path.exists())
