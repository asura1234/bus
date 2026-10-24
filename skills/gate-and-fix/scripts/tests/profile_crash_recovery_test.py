import os
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from coverage_run import MARKER, Run  # noqa: E402


class ProfileCrashRecoveryTest(unittest.TestCase):
    def test_next_gate_removes_a_run_left_by_a_killed_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            target = root / "target"
            output = root / "temp/gate-and-fix/coverage"
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}, clear=False):
                os.environ.pop("CARGO_LLVM_COV_TARGET_DIR", None)
                os.environ.pop("BUS_GATE_PROFILE_DIR", None)
                # An actual dead owner distinguishes crashes from concurrent live gates.
                code = (
                    f"import sys;sys.path.insert(0,{str(Path(__file__).resolve().parents[1])!r});"
                    "from pathlib import Path;from coverage_run import MARKER, Run;"
                    f"r=Run(Path({str(root)!r}),Path({str(output)!r}),'gate');r.__enter__();"
                    "(r.path/'bus-unit-1_0.profraw').write_bytes(b'x'*1024);"
                    "print(r.path,flush=True);import time;time.sleep(60)"
                )
                proc = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE,
                                        text=True)
                try:
                    leaked = Path(proc.stdout.readline().strip())
                finally:
                    proc.kill()
                    proc.wait(timeout=5)
                    proc.stdout.close()
                with Run(root, output, "gate"):
                    pass
            self.assertFalse(
                leaked.exists(),
                f"a killed gate's run directory is never reclaimed: {sorted(p.name for p in leaked.parent.glob("run-*"))}",
            )




class PersistentCoverageBuildTest(unittest.TestCase):
    def test_separate_runs_keep_the_instrumented_cache_and_profile_only_in_the_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            target = root / "target/llvm-cov-target"
            cache = target / "debug/deps/instrumented-cache"
            cache.parent.mkdir(parents=True)
            cache.write_bytes(b"compiled")
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=True):
                for _ in range(2):
                    with Run(root, root / "reports", "gate") as run:
                        self.assertEqual(Path(os.environ["CARGO_LLVM_COV_TARGET_DIR"]), target)
                        (run.path / "fixture.profraw").write_bytes(b"data")
                    self.assertFalse(run.path.exists())
                    self.assertEqual(cache.read_bytes(), b"compiled")


class ProfileOwnerGuardsTest(unittest.TestCase):
    def test_live_owner_is_not_reaped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=True):
                first = Run(root, root / "reports", "gate")
                first.__enter__()
                (first.path / "keep.profraw").write_bytes(b"live")
                os.environ.clear()
                os.environ.update(first.original)
                try:
                    with Run(root, root / "reports", "gate"):
                        self.assertEqual((first.path / "keep.profraw").read_bytes(), b"live")
                finally:
                    first.__exit__(None, None, None)

    def test_only_valid_checkout_owned_run_markers_are_reaped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}, clear=True):
                first = Run(root, root / "reports", "gate")
                first.__enter__()
                original = json.loads((first.path / MARKER).read_text())
                os.environ.clear()
                os.environ.update(first.original)
                # All have dead/reused owners; only one is eligible for deletion.
                cases = ["dead", "foreign", "bad-name", "bad-marker", "copied-marker"]
                paths = []
                for n, case in enumerate(cases):
                    path = first.path.parent / ("run-" + f"{n + 10:032x}")
                    if case == "bad-name":
                        path = first.path.parent / "run-unowned"
                    path.mkdir()
                    (path / "keep.profraw").write_bytes(b"counters")
                    data = {**original, "path": str(path), "start": "previous incarnation"}
                    if case == "foreign":
                        data["root"] = str(root / "other-checkout")
                    if case == "copied-marker":
                        data = original
                    (path / MARKER).write_text("not JSON" if case == "bad-marker" else json.dumps(data))
                    paths.append(path)
                try:
                    with Run(root, root / "reports", "gate"):
                        self.assertFalse(paths[0].exists(), "a reused PID must count as dead")
                        for path in paths[1:]:
                            self.assertEqual((path / "keep.profraw").read_bytes(), b"counters")
                        self.assertTrue(first.path.exists())
                finally:
                    first.__exit__(None, None, None)


if __name__ == "__main__":
    unittest.main()
