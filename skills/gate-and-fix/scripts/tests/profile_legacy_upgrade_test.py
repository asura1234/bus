import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from coverage_run import Run  # noqa: E402
from profile_report import merged_profile  # noqa: E402


class ProfileLegacyUpgradeTest(unittest.TestCase):
    def test_first_gate_after_upgrade_reports_despite_flat_data_from_the_old_gate(self):
        # Every worktree gated before this change holds flat raws and the
        # workspace-named profdata in its persistent llvm-cov target.
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target"),
                                         "LLVM_PROFDATA": "fixture-profdata"}, clear=True):
                base = root / "target" / "llvm-cov-target"
                base.mkdir(parents=True)
                (base / f"{root.name}-unit-123-456_0.profraw").write_bytes(b"old raw")
                (base / f"{root.name}.profdata").write_bytes(b"old merged")
                cache = base / "debug/deps/compiled"
                cache.parent.mkdir(parents=True)
                cache.write_bytes(b"instrumented cache")
                with Run(root, root / "reports", "gate") as run:
                    self.assertFalse(list(base.glob("*.profraw")))
                    self.assertFalse((base / f"{root.name}.profdata").exists())
                    self.assertEqual(cache.read_bytes(), b"instrumented cache")
                    (run.path / "unit.profraw").write_bytes(b"current counters")

                    def merge(*argv):
                        Path(argv[argv.index("-o") + 1]).write_bytes(b"current counters")
                        return 0

                    try:
                        with merged_profile(root, run.base, dict(os.environ), merge):
                            self.assertEqual((base / f"{root.name}.profdata").read_bytes(), b"current counters")
                    except ValueError as error:
                        self.fail(f"first gate after upgrade cannot report: {error}")
                self.assertFalse(list(base.glob("*.profraw")))
                self.assertFalse((base / f"{root.name}.profdata").exists())
                self.assertEqual(cache.read_bytes(), b"instrumented cache")

    def test_upgrade_cleanup_preserves_a_live_runs_profiles_and_report_lookup(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=True):
                live = Run(root, root / "reports", "gate")
                live.__enter__()
                merged = live.path / "merged.profdata"
                merged.write_bytes(b"live index")
                raw = live.path / "unit.profraw"
                raw.write_bytes(b"live counters")
                index = live.base / (root.name + ".profdata")
                os.link(merged, index)
                os.environ.clear()
                os.environ.update(live.original)
                try:
                    with Run(root, root / "reports", "gate"):
                        self.assertEqual(index.read_bytes(), b"live index")
                        self.assertTrue(index.samefile(merged))
                        self.assertEqual(raw.read_bytes(), b"live counters")
                finally:
                    live.__exit__(None, None, None)


if __name__ == "__main__":
    unittest.main()
