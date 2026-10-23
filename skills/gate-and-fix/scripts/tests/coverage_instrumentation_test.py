import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402


class CoverageInstrumentationTest(unittest.TestCase):
    def test_cli_profiles_cannot_mask_a_missing_unit_run(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = root / "target"
            target.mkdir()

            def run(*argv, **kwargs):
                if argv[:3] == ("cargo", "llvm-cov", "run"):
                    (target / f"{root.name}-cli-fixture.profraw").write_bytes(b"cli")
                return 0

            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "OUTPUT", root / "reports"),
                patch.object(quality, "head", return_value="fixture"),
                patch.object(quality, "python_files", return_value=[root / "one_test.py"]),
                patch.object(quality, "rust_test", return_value=0),
                patch.object(quality, "run", side_effect=run),
                patch.dict(os.environ, {"CARGO_LLVM_COV_TARGET_DIR": str(target)}),
            ):
                self.assertEqual(quality.unit(), 1)
                value = json.loads((quality.OUTPUT / "state.json").read_text())
                self.assertFalse(value["unit"])
                # The coverage lane must independently reject missing unit
                # evidence even if other lanes/CLI profiles appear successful.
                value.update(unit=True, integration=True)
                quality.save_state(value)
                with self.assertRaisesRegex(ValueError, "unit.*profraw"):
                    quality.coverage()

    def test_missing_or_removed_unit_profiles_stop_export(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = root / "target"
            target.mkdir()
            name = f"{root.name}-unit-fixture.profraw"
            value = {
                "head": "fixture", "unit": True, "integration": True,
                "profile_target": str(target.resolve()), "unit_profraw": [name],
            }
            (target / f"{root.name}-cli-fixture.profraw").write_bytes(b"cli")
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "OUTPUT", root),
                patch.object(quality, "state", return_value=value),
                patch.object(quality, "run") as run,
                patch.dict(os.environ, {"CARGO_LLVM_COV_TARGET_DIR": str(target)}),
                self.assertRaisesRegex(ValueError, "unit.*profraw"),
            ):
                quality.coverage()
            run.assert_not_called()

    def test_large_drop_is_explicit_and_cannot_replace_the_successful_reference(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = root / "target"
            target.mkdir()
            name = f"{root.name}-unit-fixture.profraw"
            (target / name).write_bytes(b"unit")
            value = {
                "head": "fixture", "unit": True, "integration": True,
                "profile_target": str(target.resolve()), "unit_profraw": [name],
            }
            policy = root / "policy.json"
            policy.write_text(json.dumps({"rust_line_percent": 80}))
            reference = root / "last-successful.json"
            reference.write_text(json.dumps({
                "head": "prior", "rust": {"percent": 89, "pass": True},
            }))
            before = reference.read_bytes()
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "OUTPUT", root),
                patch.object(quality, "POLICY", policy),
                patch.object(quality, "state", return_value=value),
                patch.object(quality, "run", return_value=0),
                patch.dict(os.environ, {"CARGO_LLVM_COV_TARGET_DIR": str(target)}),
            ):
                with patch.object(quality, "rust_lines", return_value=(68, 100)):
                    with self.assertRaisesRegex(ValueError, "coverage dropped.*21"):
                        quality.coverage()
                self.assertEqual(reference.read_bytes(), before)
                with patch.object(quality, "rust_lines", return_value=(69, 100)):
                    self.assertEqual(quality.coverage(), 1)  # exactly 20, ordinary floor failure
                with patch.object(quality, "rust_lines", return_value=(90, 100)):
                    self.assertEqual(quality.coverage(), 0)
                self.assertEqual(json.loads(reference.read_text())["rust"]["percent"], 90)

    @unittest.skipUnless(
        shutil.which("cargo") and shutil.which("cargo-llvm-cov"),
        "requires the Rust coverage tools",
    )
    def test_normal_test_build_cannot_mask_coverage(self):
        for shared_target in (False, True):
            with self.subTest(shared_target=shared_target):
                self.exercise_target_cache(shared_target)

    def exercise_target_cache(self, shared_target):
        # A dependency-free fixture exercises Cargo's real artifact cache. Its
        # private target and profiles never touch the enclosing gate's coverage.
        with tempfile.TemporaryDirectory(prefix="bus-cov-cache-") as folder:
            root = Path(folder)
            target = root / "target"
            coverage_target = target if shared_target else target / "llvm-cov-target"
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "coverage_cache_fixture"\n'
                'version = "0.0.0"\nedition = "2021"\n[workspace]\n'
            )
            (root / "src/lib.rs").write_text(
                'pub fn answer() -> u8 { 42 }\n'
                '#[cfg(test)] mod tests { #[test] fn answer_is_covered() { '
                'assert_eq!(super::answer(), 42); } }\n'
            )
            shutil.copyfile(quality.ROOT / "rust-toolchain.toml", root / "rust-toolchain.toml")
            env = {
                key: value for key, value in os.environ.items()
                if not key.startswith(("BUS_", "HERDR_", "CARGO_LLVM_COV", "__CARGO_LLVM_COV"))
                and key not in {
                    "RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                    "LLVM_PROFILE_FILE", "LLVM_PROFILE_FILE_NAME",
                }
            }
            env.update(
                BUS_DATA_DIR=str(root / "data"),
                CARGO_TARGET_DIR=str(target),
                CARGO_LLVM_COV_TARGET_DIR=str(coverage_target),
                CARGO_ENCODED_RUSTFLAGS="",
                CARGO_INCREMENTAL="0",
            )
            subprocess.run(
                ["cargo", "generate-lockfile", "--offline"], cwd=root, env=env,
                check=True, capture_output=True, timeout=30,
            )
            subprocess.run(
                ["cargo", "test", "--locked", "--lib", "--no-run"],
                cwd=root, env=env, check=True, capture_output=True, timeout=30,
            )
            real_which = shutil.which
            modes = [False, True] if real_which("cargo-nextest") else [False]
            for nextest in modes:
                def which(program):
                    if program == "cargo-nextest" and not nextest:
                        return None
                    return real_which(program)

                with (
                    self.subTest(nextest=nextest),
                    patch.object(quality, "ROOT", root),
                    patch.dict(os.environ, env, clear=True),
                    patch.object(quality.shutil, "which", side_effect=which),
                ):
                    self.assertEqual(quality.rust_test("--lib", fresh=True), 0)
                    profiles = quality.unit_profraw_files(coverage_target)
                    self.assertTrue(
                        profiles, "coverage reused a normal binary and wrote no unit profile"
                    )
                    self.assertTrue(all(path.stat().st_size > 0 for path in profiles))
                    quality.require_unit_profraw({
                        "profile_target": str(coverage_target.resolve()),
                        "unit_profraw": [path.name for path in profiles],
                    })
