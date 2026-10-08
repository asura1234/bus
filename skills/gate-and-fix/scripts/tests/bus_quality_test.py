import json
import os
import re
import runpy
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402


class BusQualityTest(unittest.TestCase):
    def test_scope_keys_activate_together_and_leave_legacy_clippy_dormant(self):
        self.assertIsNone(quality.configured_test_scopes({}))
        self.assertEqual(quality.clippy_commands({}), ((
            "cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings",
        ),))
        for policy in ({"test_files": []}, {"test_dirs": []},
                       {"test_files": "*_test.*", "test_dirs": []},
                       {"production_clippy_lints": ["clippy::unwrap_used"]}):
            with self.subTest(policy=policy), self.assertRaises(ValueError):
                quality.clippy_commands(policy)

    def test_selected_production_lints_run_before_test_target_allowances(self):
        policy = {"test_files": ["*_test.*"], "test_dirs": ["tests"],
                  "production_clippy_lints": ["clippy::too_many_lines",
                                               "clippy::cognitive_complexity"]}
        production, tests = quality.clippy_commands(policy)
        self.assertEqual(production[:6], ("cargo", "clippy", "--bin", "bus", "--locked", "--"))
        self.assertEqual(tests[:5], ("cargo", "clippy", "--all-targets", "--locked", "--"))
        self.assertNotIn("-A", production)
        self.assertEqual(production[-4:], ("-D", "clippy::too_many_lines", "-D", "clippy::cognitive_complexity"))
        self.assertEqual(tests[-4:], ("-A", "clippy::too_many_lines", "-A", "clippy::cognitive_complexity"))

    def test_production_counting_waits_for_final_scope_keys(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "src/example.rs"
            source.parent.mkdir()
            source.write_text("// production\n\nfn product() {}\n#[cfg(all(test, unix))]\nmod tests {\nfn case() {}\n}\n")
            test = root / "src/nested/tests/helpers.rs"
            test.parent.mkdir(parents=True)
            test.write_text("line\n" * 12)
            python = root / "tools/example_test.py"
            python.parent.mkdir()
            python.write_text("line\n" * 12)
            policy = root / "lint.toml"
            legacy = "max_file_lines = 3\ngenerated_files = []\n"
            with patch.object(quality, "ROOT", root), patch.object(quality, "LINT_POLICY", policy):
                policy.write_text(legacy)
                self.assertEqual(quality.file_lengths(), 1)
                policy.write_text(legacy + 'test_files = ["*_test.*"]\ntest_dirs = ["tests"]\n')
                self.assertEqual(quality.file_lengths(), 0)
                source.write_text(source.read_text() + "fn missed() {}\n")
                self.assertEqual(quality.file_lengths(), 1)

    def test_final_coverage_scopes_exclude_combined_cfg_and_suffix_files(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "src/example.rs"
            source.parent.mkdir()
            source.write_text("fn missed() {}\n#[cfg(all(test, unix))]\nmod tests {\nfn case() {}\n}\nfn after() {}\n")
            test = root / "src/helper_test.rs"
            test.write_text("fn case() {}\n")
            report = root / "report.lcov"
            report.write_text(f"SF:{source}\nDA:1,0\nDA:4,1\nDA:6,0\nend_of_record\nSF:{test}\nDA:1,1\nend_of_record\n")
            policy = root / "lint.toml"
            with patch.object(quality, "ROOT", root), patch.object(quality, "LINT_POLICY", policy):
                policy.write_text("generated_files = []\n")
                self.assertEqual(quality.rust_lines(report), (2, 4))
                policy.write_text('generated_files = []\ntest_files = ["*_test.*"]\ntest_dirs = ["tests"]\n')
                self.assertEqual(quality.rust_lines(report), (0, 2))

    def test_unit_builds_cli_and_uses_it_instead_of_a_stale_binary(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "OUTPUT", root / "profiles"),
                patch.object(quality, "head", return_value="fixture"),
                patch.object(
                    quality, "python_files", return_value=[root / "scripts/one_test.py"]
                ),
                patch.object(quality, "rust_test", return_value=0) as rust_test,
                patch.object(quality, "run", return_value=0) as run,
                patch.dict(os.environ, {"BUS_TEST_BINARY": "/stale/bus"}),
            ):
                self.assertEqual(quality.unit(), 0)
                rust_test.assert_called_once_with(
                    "--bin", "bus", fresh=True, in_process_server=False
                )
                self.assertEqual(
                    run.call_args_list[0].args[:3], ("cargo", "llvm-cov", "run")
                )
                self.assertEqual(run.call_args_list[0].args[-1], "--help")
                env = run.call_args.kwargs["env"]
                self.assertNotEqual(env["BUS_TEST_BINARY"], "/stale/bus")
                self.assertTrue(
                    env["BUS_TEST_BINARY"].endswith(
                        "/debug/bus.exe" if os.name == "nt" else "/debug/bus"
                    )
                )
                self.assertEqual(
                    env.get("BUS_DATA_DIR"), os.environ.get("BUS_DATA_DIR")
                )
                self.assertIn("%p", env["LLVM_PROFILE_FILE"])
                # Python tests run plainly: no coverage.py, no Python floor.
                self.assertEqual(
                    run.call_args.args[:4], (sys.executable, "-m", "pytest", "-q")
                )

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

    def test_python_roots_discover_tools_and_packaging_and_skip_missing_directories(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            expected = []
            for name in ("tools", "packaging"):
                source = root / name / "nested/module.py"
                source.parent.mkdir(parents=True)
                source.write_text("value = 1\n")
                expected.append(source)
            (root / "scripts").write_text("not a directory")
            with patch.object(quality, "ROOT", root):
                self.assertEqual(quality.python_roots(), ("tools", "packaging"))
                self.assertEqual(quality.python_files(), sorted(expected))

    def test_generated_rust_exclusions_follow_the_policy_and_escape_paths(self):
        for generated in (["src/generated.v1[ffi].rs"], []):
            with (
                self.subTest(generated=generated),
                patch.object(
                    Path,
                    "read_text",
                    autospec=True,
                    return_value="generated_files = " + json.dumps(generated),
                ) as read,
            ):
                loaded = runpy.run_path(quality.__file__)
                read.assert_called_once_with(quality.LINT_POLICY, encoding="utf-8")
                self.assertEqual(loaded["GENERATED_RUST"], tuple(generated))
                pattern = loaded["RUST_EXCLUDE"]
                for excluded in (
                    "/repo/tests/integration.rs",
                    "/repo/vendor/library.rs",
                    "/repo/src/module/tests.rs",
                    "/repo/src/module/test_support.rs",
                    "/repo/build.rs",
                    *("/repo/" + name for name in generated),
                ):
                    self.assertRegex(excluded, pattern)
                for included in (
                    "/repo/src/production.rs",
                    "/repo/src/generatedXv1f.rs",
                    "/repo/src/generated.v1[ffi].rs.extra",
                ):
                    self.assertNotRegex(included, pattern)

    def test_file_length_cap_works_after_the_exemption_table_is_removed(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "src/production.rs"
            source.parent.mkdir()
            source.write_text("line\n" * 801)
            policy = root / "lint.toml"
            policy.write_text("max_file_lines = 800\ngenerated_files = []\n")
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "LINT_POLICY", policy),
            ):
                self.assertEqual(quality.file_lengths(), 1)

    def test_lint_runs_the_architecture_suites_and_prints_report_without_enforcement(self):
        with (
            patch.object(quality, "python_roots", return_value=("tools", "skills")),
            patch.object(quality, "file_lengths", return_value=0),
            patch.object(quality, "run", return_value=0) as run,
        ):
            self.assertEqual(quality.lint(), 0)
            calls = [call.args for call in run.call_args_list]
            self.assertIn(
                (
                    sys.executable,
                    "-m",
                    "ruff",
                    "check",
                    "--isolated",
                    "--select",
                    "E9,F",
                    "tools",
                    "skills",
                ),
                calls,
            )
            self.assertIn(
                (
                    sys.executable,
                    "-m",
                    "pytest",
                    "-q",
                    "tools/tests/ui_hot_path_test.py",
                    "tools/tests/import_boundaries_test.py",
                    "tools/tests/scopes_test.py",
                    "tools/tests/test_placement_check_test.py",
                ),
                calls,
            )
            self.assertIn(
                (sys.executable, "-m", "tools.quality.import_boundaries"), calls
            )
            self.assertIn(
                (sys.executable, "-m", "tools.quality.placement", "--enforce"), calls
            )
            # Import boundaries remain report-only; test placement is enforced.
            self.assertNotIn(
                (sys.executable, "-m", "tools.quality.import_boundaries", "--enforce"),
                calls,
            )

    def test_lint_rejects_an_empty_python_inventory_instead_of_scanning_the_cwd(self):
        with (
            patch.object(quality, "python_roots", return_value=()),
            patch.object(quality, "run") as run,
            self.assertRaisesRegex(ValueError, "no Python source roots"),
        ):
            quality.lint()
        run.assert_not_called()

    def test_integration_uses_the_shared_server_prefix_selection(self):
        with (
            patch.object(quality, "state", return_value={"unit": True}),
            patch.object(quality, "save_state") as save,
            patch.object(quality, "rust_test", return_value=0) as rust_test,
        ):
            self.assertEqual(quality.integration(), 0)
            self.assertEqual(rust_test.call_args_list[0].args, ("--test", "*"))
            self.assertEqual(rust_test.call_args_list[1].args, ("--bin", "bus"))
            self.assertEqual(
                rust_test.call_args_list[1].kwargs, {"in_process_server": True}
            )
            self.assertTrue(save.call_args.args[0]["integration"])

    def test_coverage_exit_status_enforces_only_the_rust_floor(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            policy = root / "policy.json"
            policy.write_text(json.dumps({"rust_line_percent": 80}))
            for rust, expected in ((79, 1), (80, 0)):
                with (
                    self.subTest(rust=rust),
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
                ):
                    self.assertEqual(quality.coverage(), expected)
                    summary = json.loads((root / "summary.json").read_text())
                    self.assertEqual(set(summary), {"head", "rust"})

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

    def test_rust_coverage_uses_the_configured_source_root_and_generated_exclusions(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "moved/production.rs"
            source.parent.mkdir()
            source.write_text("fn missed() {}\n")
            generated = root / "moved/generated.rs"
            report = root / "report.lcov"
            report.write_text(
                f"SF:{source}\nDA:1,0\nend_of_record\n"
                f"SF:{generated}\nDA:1,1\nend_of_record\n"
            )
            with (
                patch.object(quality, "ROOT", root),
                patch.object(quality, "RUST_SOURCE_ROOT", "moved"),
                patch.object(quality, "RUST_EXCLUDE", r"/moved/generated\.rs$"),
            ):
                self.assertEqual(quality.rust_lines(report), (0, 1))

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
        for root in quality.PYTHON_ROOTS:
            for name, expected in (
                ("nested/suite_test.py", True),
                ("tests/helper_test.py", True),
                ("test_legacy.py", False),
                ("tests/helper.py", False),
                ("production.py", False),
            ):
                with self.subTest(root=root, name=name):
                    self.assertEqual(quality.is_test(Path(root) / name), expected)
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
            args = (
                command[3:]
                if command[1:3] in (["-m", "unittest"], ["-m", "pytest"])
                else command[1:]
            )
            for arg in args:
                path = (
                    arg.replace(".", "/") + ".py"
                    if arg.startswith(("scripts.", "tools."))
                    else arg
                )
                self.assertIn(path, files)
        self.assertIn("tools/tests/acceptance_live_ui_test.py", files)
        self.assertIn("skills/gate-and-fix/scripts/tests/bus_quality_test.py", files)

    def test_optional_nextest_has_a_real_cargo_fallback(self):
        for nextest in (False, True):
            with (
                patch.object(
                    quality.shutil, "which", return_value="nextest" if nextest else None
                ),
                patch.object(quality, "run", return_value=0) as run,
            ):
                quality.rust_test("--bin", "bus", fresh=True, in_process_server=False)
                self.assertEqual(
                    run.call_args.args[2], "nextest" if nextest else "test"
                )
                self.assertIn("--no-report", run.call_args.args)
                self.assertIn("--locked", run.call_args.args)
                self.assertNotIn("--no-clean", run.call_args.args)
                if not nextest:
                    self.assertEqual(
                        run.call_args.args[-3:], ("--", "--skip", "server::tests::")
                    )

    def test_server_prefix_changes_apply_to_nextest_and_libtest_in_both_lanes(self):
        prefix = "server::tests::"
        for nextest in (False, True):
            for selection in (False, True, None):
                with (
                    self.subTest(nextest=nextest, selection=selection),
                    patch.object(quality, "IN_PROCESS_SERVER_TESTS", prefix),
                    patch.object(
                        quality.shutil, "which", return_value="nextest" if nextest else None
                    ),
                    patch.object(quality, "run", return_value=0) as run,
                ):
                    quality.rust_test("--bin", "bus", in_process_server=selection)
                    args = run.call_args.args
                    if selection is None:
                        self.assertNotIn("-E", args)
                        self.assertNotIn("--skip", args)
                        self.assertNotIn(prefix, args)
                    elif nextest:
                        expected = f"test(/^{re.escape(prefix)}/)"
                        self.assertEqual(
                            args[-2:], ("-E", expected if selection else f"not {expected}")
                        )
                    elif selection:
                        self.assertEqual(args[-1], prefix)
                    else:
                        self.assertEqual(args[-3:], ("--", "--skip", prefix))


if __name__ == "__main__":
    unittest.main()
