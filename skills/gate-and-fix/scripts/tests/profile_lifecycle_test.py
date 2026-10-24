import contextlib
import io
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402
import gate_and_fix as gate  # noqa: E402


def repository(root):
    for argv in (
        ("init", "-q"), ("config", "user.email", "fixture@example.test"),
        ("config", "user.name", "fixture"),
    ):
        subprocess.run(("git", *argv), cwd=root, check=True)
    (root / ".gitignore").write_text("target/\ntemp/\n")
    (root / "input.txt").write_text("base\n")
    subprocess.run(("git", "add", ".gitignore", "input.txt"), cwd=root, check=True)
    subprocess.run(("git", "commit", "-qm", "base"), cwd=root, check=True)
    base = subprocess.check_output(("git", "rev-parse", "HEAD"), cwd=root, text=True).strip()
    (root / "input.txt").write_text("head\n")
    subprocess.run(("git", "commit", "-qam", "head"), cwd=root, check=True)
    return base

class ProfileLifecycleTest(unittest.TestCase):
    @contextlib.contextmanager
    def fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            output = root / "reports"
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "OUTPUT", output),
                patch.object(quality.importlib.util, "find_spec", return_value=object()),
                patch.object(quality.shutil, "which", return_value="/tool"),
                patch.dict(os.environ, {
                    "CARGO_TARGET_DIR": str(target),
                    "CARGO_LLVM_COV_TARGET_DIR": str(target),
                }),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                yield root, target, output

    def emit(self, stage):
        target = quality.llvm_cov_target(os.environ)
        target.mkdir(parents=True, exist_ok=True)
        (target / f"fixture-{stage}.profraw").write_bytes(b"counters")
        return target

    def test_ci_exports_before_removing_one_private_profile_directory(self):
        with self.fixture() as (_, target, output):
            locations = []

            def unit():
                locations.append(self.emit("unit"))
                return 0

            def integration():
                locations.append(self.emit("integration"))
                return 0

            def coverage():
                location = quality.llvm_cov_target(os.environ)
                self.assertTrue((location / "fixture-unit.profraw").is_file())
                self.assertTrue((location / "fixture-integration.profraw").is_file())
                (location / "fixture.profdata").write_bytes(b"merged")
                output.mkdir(parents=True, exist_ok=True)
                (output / "summary.json").write_text('{"covered":89,"total":100}')
                return 0

            with (
                patch.object(quality, "lint", return_value=0),
                patch.object(quality, "unit", side_effect=unit),
                patch.object(quality, "integration", side_effect=integration),
                patch.object(quality, "coverage", side_effect=coverage),
            ):
                self.assertEqual(quality.main(["ci"]), 0)
            self.assertFalse(list(target.rglob("*.profraw")))
            self.assertFalse(list(target.rglob("*.profdata")))
            self.assertEqual(len(set(locations)), 1)
            self.assertNotEqual(locations[0], target)
            self.assertFalse(locations[0].exists())
            self.assertEqual(json.loads((output / "summary.json").read_text())["covered"], 89)
            self.assertEqual(os.environ["CARGO_LLVM_COV_TARGET_DIR"], str(target))

    def test_failure_exception_and_keyboard_interrupt_remove_current_profiles(self):
        for error in (None, ValueError("fixture failure"), KeyboardInterrupt()):
            with self.subTest(error=type(error).__name__), self.fixture() as (_, target, _):
                def unit():
                    self.emit("unit")
                    if error is not None:
                        raise error
                    return 1

                with patch.object(quality, "unit", side_effect=unit):
                    try:
                        self.assertNotEqual(quality.main(["unit"]), 0)
                    except KeyboardInterrupt:
                        pass
                self.assertFalse(list(target.rglob("*.profraw")))

    def test_standalone_lanes_share_profiles_until_coverage_then_remove_them(self):
        with self.fixture() as (_, target, output):
            locations = []

            def lane(name):
                locations.append(self.emit(name))
                if name == "coverage":
                    output.mkdir(parents=True, exist_ok=True)
                    (output / "summary.json").write_text("{}")
                return 0

            with (
                patch.object(quality, "unit", side_effect=lambda: lane("unit")),
                patch.object(quality, "integration", side_effect=lambda: lane("integration")),
                patch.object(quality, "coverage", side_effect=lambda: lane("coverage")),
            ):
                self.assertEqual(quality.main(["unit"]), 0)
                self.assertTrue((locations[0] / "fixture-unit.profraw").exists())
                self.assertEqual(quality.main(["integration"]), 0)
                self.assertEqual(quality.main(["coverage"]), 0)
            self.assertEqual(len(set(locations)), 1)
            self.assertNotEqual(locations[0], target)
            self.assertFalse(list(target.rglob("*.profraw")))
            self.assertFalse(locations[0].exists())
            self.assertTrue(output.is_dir())

    def test_coverage_preflight_failure_cleans_the_retained_run(self):
        with self.fixture() as (_, target, _):
            with patch.object(quality, "unit", side_effect=lambda: (self.emit("unit"), 0)[1]):
                self.assertEqual(quality.main(["unit"]), 0)
            self.assertTrue(list(target.rglob("*.profraw")))
            with patch.object(quality.importlib.util, "find_spec", return_value=None):
                self.assertEqual(quality.main(["coverage"]), 1)
            self.assertFalse(list(target.rglob("*.profraw")))

    def test_cleanup_preserves_unrelated_files_in_the_configured_target(self):
        with self.fixture() as (_, target, _):
            unrelated = target / "another-owner/keep.profraw"
            unrelated.parent.mkdir(parents=True)
            unrelated.write_bytes(b"untouched")
            with patch.object(quality, "unit", side_effect=lambda: (self.emit("unit"), 1)[1]):
                self.assertEqual(quality.main(["unit"]), 1)
            self.assertEqual(unrelated.read_bytes(), b"untouched")
            self.assertEqual(list(target.rglob("*.profraw")), [unrelated])

    def test_restarting_units_discards_the_previous_run(self):
        with self.fixture() as (_, target, _):
            locations = []

            def unit():
                locations.append(self.emit("unit"))
                return 0

            with patch.object(quality, "unit", side_effect=unit):
                self.assertEqual(quality.main(["unit"]), 0)
                self.assertEqual(quality.main(["unit"]), 0)
            self.assertNotEqual(locations[0], locations[1])
            self.assertFalse(locations[0].exists())
            with patch.object(quality, "coverage", return_value=1):
                self.assertEqual(quality.main(["coverage"]), 1)
            self.assertFalse(list(target.rglob("*.profraw")))

    def test_corrupted_ownership_cannot_delete_unrelated_source(self):
        with self.fixture() as (root, _, output):
            source = root / "source"
            source.mkdir()
            keep = source / "keep.txt"
            keep.write_text("untouched")
            output.mkdir()
            (output / "active-coverage-run.json").write_text(json.dumps({
                "root": str(root.resolve()), "base": str(root.resolve()),
                "path": str(source.resolve()),
            }))
            self.assertEqual(quality.main(["unit"]), 1)
            self.assertEqual(keep.read_text(), "untouched")

    def test_complete_gate_exports_artifact_without_leaving_raw_profiles(self):
        self.exercise_complete_gate(failed=False)

    def test_failed_gate_records_failure_and_removes_its_profiles(self):
        self.exercise_complete_gate(failed=True)

    def exercise_complete_gate(self, *, failed):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = repository(root)
            script = (
                "import os;from pathlib import Path;"
                "p=Path(os.environ['CARGO_LLVM_COV_TARGET_DIR']);p.mkdir(parents=True,exist_ok=True);"
                "(p/'fixture.profraw').write_bytes(b'counters');"
                "(p/'fixture.profdata').write_bytes(b'merged')"
            )
            gates = [gate.Gate(name, (sys.executable, "-c", script),
                               requires_exclusive_execution=True)
                     for name in ("lint", "unit", "integration", "coverage")]
            if failed:
                gates[1] = gate.Gate("unit", (sys.executable, "-c", script + ";raise SystemExit(1)"),
                                     requires_exclusive_execution=True)
            gates.append(gate.Gate("diff-check", ("git", "diff", "--check", f"{base}...HEAD")))
            stdout = io.StringIO()
            with (
                patch.object(gate, "select_gates", return_value=gates),
                patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target"),
                                       "CARGO_LLVM_COV_TARGET_DIR": str(root / "target")}),
                contextlib.redirect_stdout(stdout),
            ):
                self.assertEqual(gate.main([
                    "--repo", str(root), "--base", base, "--round", "1",
                    "--artifact-root", str(root / "temp/artifacts"),
                ]), int(failed))
            artifact = Path(stdout.getvalue().strip())
            self.assertEqual(gate.validate_round(artifact.read_text(), expected_base=base),
                             "FAIL" if failed else "PASS")
            self.assertFalse(list((root / "target").rglob("*.profraw")))
            self.assertFalse(list((root / "target").rglob("*.profdata")))

    @unittest.skipUnless(os.name == "posix", "POSIX signal and process-group contract")
    def test_sigterm_stops_instrumented_children_before_profile_cleanup(self):
        self.exercise_interrupt("quality")

    @unittest.skipUnless(os.name == "posix", "POSIX signal and process-group contract")
    def test_sigterm_gate_runner_stops_children_and_removes_profiles(self):
        self.exercise_interrupt("gate")

    @unittest.skipUnless(os.name == "posix", "POSIX signal and process-group contract")
    def test_sigterm_nested_quality_lane_finishes_before_parent_cleanup(self):
        self.exercise_interrupt("nested")

    def exercise_interrupt(self, mode):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ready = root / "ready.json"
            child = (
                "import json,os,signal,time;from pathlib import Path;signal.signal(signal.SIGTERM,signal.SIG_IGN);"
                "p=Path(os.environ['CARGO_LLVM_COV_TARGET_DIR']);p.mkdir(parents=True,exist_ok=True);"
                "(p/'unit.profraw').write_bytes(b'counters');"
                f"Path({str(ready)!r}).write_text(json.dumps({{'pid':os.getpid(),'pgid':os.getpgrp()}}));"
                "time.sleep(60)"
            )
            script = (
                f"import sys;sys.path.insert(0,{str(Path(quality.__file__).parent)!r});"
                "import bus_quality as q;from pathlib import Path;"
                f"q.ROOT=Path({str(root)!r});q.OUTPUT=q.ROOT/'reports';"
                f"q.unit=lambda:q.run(sys.executable,'-c',{child!r});"
                "sys.exit(q.main(['unit']))"
            )
            if mode != "quality":
                base = repository(root)
                command = child if mode == "gate" else script
                gate_args = ["--repo", str(root), "--base", base, "--round", "1",
                             "--artifact-root", str(root / "temp/artifacts")]
                script = (
                    f"import sys;sys.path.insert(0,{str(Path(quality.__file__).parent)!r});"
                    "import gate_and_fix as g;"
                    f"g.select_gates=lambda *a,**kw:[g.Gate('unit',(sys.executable,'-c',{command!r}),requires_exclusive_execution=True)];"
                    f"sys.exit(g.main({gate_args!r}))"
                )
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("BUS_", "HERDR_"))}
            env.update(CARGO_TARGET_DIR=str(root / "target"),
                       CARGO_LLVM_COV_TARGET_DIR=str(root / "target"))
            proc = subprocess.Popen((sys.executable, "-c", script), env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    start_new_session=True)
            groups = {proc.pid}
            try:
                deadline = time.monotonic() + 10
                while not ready.exists() and proc.poll() is None and time.monotonic() < deadline:
                    time.sleep(.02)
                self.assertTrue(ready.exists(), proc.communicate(timeout=1) if proc.poll() is not None else "no child")
                identity = json.loads(ready.read_text())
                groups.add(identity["pgid"])
                proc.send_signal(signal.SIGTERM)
                time.sleep(.05)
                if proc.poll() is None:
                    proc.send_signal(signal.SIGTERM)
                proc.communicate(timeout=10)
                self.assertNotEqual(proc.returncode, 0)
                self.assertFalse(list((root / "target").rglob("*.profraw")))
                with self.assertRaises(ProcessLookupError):
                    os.kill(identity["pid"], 0)
            finally:
                for group in groups:
                    try:
                        os.killpg(group, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                proc.wait(timeout=5)
