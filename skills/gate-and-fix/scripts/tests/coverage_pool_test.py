"""Real gate pipeline regression for bounded, lossless concurrent profiles."""
import contextlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402
from coverage_run import PROFILE_DIR, Run
from profile_report import merged_profile  # noqa: E402


@unittest.skipUnless(
    shutil.which("cargo") and shutil.which("cargo-llvm-cov")
    and shutil.which("cargo-nextest"), "requires the Rust coverage tools and nextest",
)
class CoveragePoolTest(unittest.TestCase):
    def test_parallel_rust_and_cli_profiles_are_bounded_without_losing_counters(self):
        # A process-keyed filename regresses disk proportional to the test count.
        # A plain shared filename regresses counters under concurrent writers.
        with tempfile.TemporaryDirectory(prefix="bus-cov-pool-") as folder:
            root = Path(folder)
            target = root / "target"
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "bus"\nversion = "0.0.0"\n'
                'edition = "2021"\n[workspace]\n'
            )
            source = root / "src/main.rs"
            source.write_text(
                'fn answer(first: bool) -> u8 {\n'
                '    if first { 42 } else { 7 }\n}\n'
                'fn main() {\n    let _ = answer(true);\n'
                '    if std::env::args().nth(1).as_deref() == Some("--help") {\n'
                '        println!("fixture help");\n'
                '    } else {\n        println!("uncovered branch");\n    }\n}\n'
                '#[cfg(test)] mod tests {\n'
                + ''.join(
                    f'#[test] fn case_{n}() {{ assert_eq!(super::answer('
                    f'{"true" if n % 2 else "false"}), {42 if n % 2 else 7}); }}\n'
                    for n in range(24)
                ) + '}\n'
            )
            test = root / "cli_test.py"
            test.write_text(
                'import concurrent.futures, os, subprocess\n'
                'def test_parallel_cli():\n'
                '    def invoke(_):\n'
                '        subprocess.run([os.environ["BUS_TEST_BINARY"], "--help"], '
                'check=True, capture_output=True)\n'
                '    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:\n'
                '        list(pool.map(invoke, range(24)))\n'
            )
            shutil.copyfile(quality.ROOT / "rust-toolchain.toml", root / "rust-toolchain.toml")
            env = {
                key: value for key, value in os.environ.items()
                if not key.startswith(("BUS_", "HERDR_", "CARGO_LLVM_COV", "__CARGO_LLVM_COV"))
                and key not in {"RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                                "LLVM_PROFILE_FILE", "LLVM_PROFILE_FILE_NAME"}
            }
            env.update(CARGO_TARGET_DIR=str(target), CARGO_INCREMENTAL="0",
                       CARGO_ENCODED_RUSTFLAGS="", CARGO_BUILD_JOBS="2",
                       RUST_TEST_THREADS="4", NEXTEST_TEST_THREADS="4")
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root,
                           env=env, check=True, capture_output=True, timeout=30)
            snapshots = []
            counts = []
            real_child_env = quality.child_env

            def process_keyed_env(**overrides):
                value = real_child_env(**overrides)
                # Retain the old process-keyed mode as the accuracy oracle.
                for key in ("LLVM_PROFILE_FILE", "LLVM_PROFILE_FILE_NAME"):
                    if key in value and "%p" not in value[key]:
                        value[key] = re.sub(r"%[1-9]?m", "%p-%m", value[key])
                return value

            for baseline, lifecycle in ((True, False), (False, False), (False, True), (False, True)):
                with (
                    patch.object(quality, "ROOT", root),
                    patch.object(quality, "OUTPUT", root / "reports"),
                    patch.object(quality, "head", return_value="fixture"),
                    patch.object(quality, "python_files", return_value=[test]),
                    patch.dict(os.environ, env, clear=True),
                    patch.object(quality, "child_env", side_effect=(
                        process_keyed_env if baseline else real_child_env)),
                    (Run(root, root / "reports", "ci") if lifecycle else contextlib.nullcontext()),
                ):
                    active_target = quality.llvm_cov_target(os.environ)
                    binary = active_target / "debug" / ("bus.exe" if os.name == "nt" else "bus")
                    before = binary.stat().st_mtime_ns if lifecycle and binary.exists() else None
                    self.assertEqual(quality.unit(), 0)
                    if before is not None:
                        self.assertEqual(binary.stat().st_mtime_ns, before, "incremental cache was rebuilt")
                    value = json.loads((quality.OUTPUT / "state.json").read_text())
                    quality.require_unit_profraw(value)
                    if lifecycle:
                        with patch.dict(os.environ, {PROFILE_DIR: str(root / "another-run")}):
                            with self.assertRaisesRegex(ValueError, "run changed"):
                                quality.require_unit_profraw(value)
                    unit_before = {path.name: path.read_bytes()
                                   for path in quality.unit_profraw_files(active_target)}
                    self.assertEqual(quality.rust_test("--bin", "bus"), 0)
                    self.assertEqual(unit_before, {path.name: path.read_bytes()
                                                 for path in quality.unit_profraw_files(active_target)})
                    quality.require_unit_profraw(value)
                    profile_dir = quality.profile_directory(active_target)
                    profiles = list(profile_dir.glob("*.profraw"))
                    counts.append(len(profiles))
                    report = root / "coverage.json"
                    with merged_profile(root, active_target, dict(os.environ), quality.run):
                        subprocess.run(
                            ["cargo", "llvm-cov", "report", "--json", "--output-path", str(report)],
                            cwd=root, env=quality.child_env(), check=True, capture_output=True, timeout=30,
                        )
                    data = json.loads(report.read_text())["data"][0]
                    files = [entry for entry in data["files"] if Path(entry["filename"]).resolve() == source.resolve()]
                    self.assertEqual(len(files), 1)
                    self.assertGreater(files[0]["summary"]["lines"]["count"],
                                       files[0]["summary"]["lines"]["covered"])
                    snapshots.append({
                        "files": files,
                        "functions": sorted((entry for entry in data["functions"]
                                             if source.resolve() in {Path(name).resolve() for name in entry["filenames"]}),
                                            key=lambda entry: entry["name"]),
                    })
                    if not baseline:
                        self.assertLessEqual(len(quality.unit_profraw_files(active_target)), 4)
                        self.assertLessEqual(len(profiles), 12)  # separate unit/integration/CLI pools
                    if not lifecycle:
                        for profile in profiles + list(active_target.glob("*.profdata")):
                            profile.unlink()
                if lifecycle:
                    self.assertFalse(profile_dir.exists(), "the exported profiles must be removed")
                    self.assertTrue(binary.is_file(), "the build cache must remain")
                    self.assertFalse(list(target.rglob("*.profraw")))
                    self.assertFalse(list(target.rglob("*.profdata")))
            self.assertGreater(counts[0], 40, "the baseline must exercise separate writers")
            self.assertEqual(snapshots[0], snapshots[1], "pooling must preserve every counter")
            for snapshot in snapshots[2:]:
                self.assertEqual(snapshots[0], snapshot, "successive run isolation must preserve every counter")
