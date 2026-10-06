import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402


class BusQualityTest(unittest.TestCase):
    def test_file_length_cap_has_only_named_exemptions(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "src"
            source.mkdir()
            (source / "legacy.rs").write_text("line\n" * 3001)
            (source / "generated.rs").write_text("line\n" * 3001)
            policy = root / "lint.toml"
            policy.write_text(
                'max_file_lines = 3000\ngenerated_files = ["src/generated.rs"]\n[file_length_exemptions]\n"src/legacy.rs" = 3001 # Split planned in restructure.\n'
            )
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "LINT_POLICY", policy),
            ):
                self.assertEqual(quality.file_lengths(), 0)
                (source / "new.rs").write_text("line\n" * 3001)
                self.assertEqual(quality.file_lengths(), 1)
                (source / "new.rs").write_text("line\n" * 3000)
                self.assertEqual(quality.file_lengths(), 0)

    def test_coverage_exit_status_enforces_both_language_floors(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            policy = root / "policy.json"
            policy.write_text(
                json.dumps(
                    {
                        "rust_line_percent": 80,
                        "python_line_percent": 80,
                        "python_exclude": {},
                    }
                )
            )
            for rust, python, expected in ((79, 80, 1), (80, 79, 1), (80, 80, 0)):
                with (
                    self.subTest(rust=rust, python=python),
                    patch.object(quality, "OUTPUT", root),
                    patch.object(quality, "POLICY", policy),
                    patch.object(
                        quality,
                        "state",
                        return_value={
                            "head": "fixture",
                            "unit": True,
                            "integration": True,
                        },
                    ),
                    patch.object(quality, "run", return_value=0),
                    patch.object(quality, "rust_lines", return_value=(rust, 100)),
                    patch.object(quality, "python_lines", return_value=(python, 100)),
                ):
                    self.assertEqual(quality.coverage(), expected)

    def test_coverage_fails_below_floor_and_for_empty_or_invalid_measurement(self):
        self.assertTrue(quality.check_percent("fixture", 81, 100, 80)["pass"])
        self.assertFalse(quality.check_percent("fixture", 79, 100, 80)["pass"])
        for covered, total, floor in ((0, 0, 80), (80, 100, 0), (80, 100, 101)):
            with self.assertRaises(ValueError):
                quality.check_percent("fixture", covered, total, floor)

    def test_rust_coverage_omits_test_module_and_keeps_uncovered_production(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "src/example.rs"
            source.parent.mkdir()
            source.write_text(
                "fn used() {}\nfn missed() {}\n#[cfg(test)]\nmod tests {\n    fn test() {}\n}\n"
            )
            report = root / "report.lcov"
            report.write_text(f"SF:{source}\nDA:1,1\nDA:2,0\nDA:5,1\nend_of_record\n")
            with patch.object(quality, "ROOT", root):
                self.assertEqual(quality.rust_lines(report), (1, 2))
            report.write_text("")
            with patch.object(quality, "ROOT", root):
                self.assertEqual(quality.rust_lines(report), (0, 0))

    def test_python_coverage_includes_never_imported_file(self):
        from coverage import Coverage

        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "unused.py").write_text("value = 1\n")
            config = root / "coverage.ini"
            config.write_text(f"[run]\ndata_file = {root / '.coverage'}\n")
            Coverage(config_file=str(config)).save()
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "CONFIG", config),
            ):
                self.assertEqual(quality.python_lines({}), (0, 1))
                self.assertEqual(
                    quality.python_lines({"scripts/unused.py": "fixture"}), (0, 0)
                )

    def test_missing_stale_or_failed_profiles_cannot_pass(self):
        with (
            tempfile.TemporaryDirectory() as folder,
            patch.object(quality, "OUTPUT", Path(folder)),
        ):
            with self.assertRaises(OSError):
                quality.state()
            (Path(folder) / "state.json").write_text(
                json.dumps({"head": "old", "unit": True, "integration": True})
            )
            with (
                patch.object(quality, "head", return_value="new"),
                self.assertRaises(ValueError),
            ):
                quality.state()
            with (
                patch.object(
                    quality, "state", return_value={"unit": False, "integration": True}
                ),
                self.assertRaises(ValueError),
            ):
                quality.coverage()

    def test_all_maintenance_and_skill_tests_are_in_the_unit_inventory(self):
        files = {
            path.relative_to(quality.ROOT).as_posix()
            for path in quality.python_files()
            if quality.is_test(path)
        }
        recipe = (
            (quality.ROOT / "justfile")
            .read_text()
            .split("\nmaintenance-test:\n", 1)[1]
            .split("\n\n", 1)[0]
        )
        import shlex

        for line in recipe.splitlines():
            command = shlex.split(line)
            for arg in command[3:] if "unittest" in command else command[1:]:
                path = (
                    arg.replace(".", "/") + ".py" if arg.startswith("scripts.") else arg
                )
                self.assertIn(path, files)
        self.assertIn("scripts/test_bus_live_integration.py", files)
        self.assertIn("skills/gate-and-fix/scripts/tests/test_bus_quality.py", files)

    def test_optional_nextest_has_a_real_cargo_fallback(self):
        for nextest in (False, True):
            with (
                patch.object(
                    quality.shutil, "which", return_value="nextest" if nextest else None
                ),
                patch.object(quality, "run", return_value=0) as run,
            ):
                quality.rust_test(
                    "--bin",
                    "bus",
                    fresh=True,
                    test_filter="not test(/^server::headless::/)",
                )
                self.assertEqual(
                    run.call_args.args[2], "nextest" if nextest else "test"
                )
                self.assertIn("--no-report", run.call_args.args)
                self.assertIn("--locked", run.call_args.args)
                self.assertNotIn("--no-clean", run.call_args.args)
                if not nextest:
                    self.assertEqual(
                        run.call_args.args[-3:], ("--", "--skip", "server::headless::")
                    )


if __name__ == "__main__":
    unittest.main()
