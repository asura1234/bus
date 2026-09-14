import base64
import contextlib
import io
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import Future
from pathlib import Path
from unittest.mock import patch


sys.path.insert(0, str(Path(__file__).resolve().parent))

from gate_and_fix import (  # noqa: E402
    Gate,
    GateResult,
    PackageIndex,
    PathSelector,
    WorkspacePackage,
    load_coverage_selector,
    load_package_index,
    load_skill_test_files,
    main,
    render_round,
    run_argv,
    run_gates,
    select_cli_test_files,
    select_bus_gates,
    select_gates,
    select_test_scope,
    validate_round,
)


_COVERAGE_SELECTOR = PathSelector(
    frozenset({"shell/packages/chromium-frame-host/src/argv.js"}),
    ("shell/packages/canvas-platform/src/", "shell/packages/video-editor/"),
)


_SKILL_TEST_FILES = {
    "skills/gate-and-fix": ("skills/gate-and-fix/scripts/test_gate_and_fix.py",),
    "skills/pr": ("skills/pr/scripts/test_pr_format_check.py",),
}


_PACKAGE_INDEX = PackageIndex(
    (
        WorkspacePackage(
            name="@liblib-ai/libtv-desktop",
            root="shell/apps/desktop",
            layers=frozenset({"unit", "integ"}),
        ),
        WorkspacePackage(
            name="@libtv/artifact-verification",
            root="shell/packages/artifact-verification",
            layers=frozenset(),
        ),
        WorkspacePackage(
            name="@libtv/video-editor-core",
            root="shell/packages/video-editor/core",
            layers=frozenset({"unit"}),
        ),
    )
)


class GateAndFixTest(unittest.TestCase):
    def test_coverage_gates_use_the_frozen_diff_base(self) -> None:
        gates = select_gates(
            ["shell/packages/canvas-platform/src/raw-input.ts", "native/media/src/service.cpp"],
            base="immutable-base",
            quality_base="merge-base",
            package_index=_PACKAGE_INDEX,
            skill_test_files=_SKILL_TEST_FILES,
            coverage_selector=_COVERAGE_SELECTOR,
            platform="darwin",
        )
        for lane in ("typescript", "native"):
            gate = next(gate for gate in gates if gate.name == f"coverage-{lane}")
            self.assertEqual(gate.argv, run_argv("test", "coverage", lane, "--base", "immutable-base"))

    def test_run_argv_uses_the_platform_runner(self) -> None:
        self.assertEqual(
            run_argv("test", "unit", os_name="posix"),
            ("./run", "test", "unit"),
        )
        self.assertEqual(
            run_argv("test", "unit", os_name="nt"),
            (
                "powershell.exe",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "run.ps1",
                "test",
                "unit",
            ),
        )

    def test_select_gates_uses_the_committed_delta_for_format_and_lint(self) -> None:
        changed_files = [
            "skills/gate-and-fix/SKILL.md",
            "shell/packages/video-editor/src/index.ts",
            "native/media/src/service.cpp",
        ]

        gates = select_gates(
            changed_files,
            base="baseline-sha",
            package_index=_PACKAGE_INDEX,
            skill_test_files=_SKILL_TEST_FILES,
            coverage_selector=_COVERAGE_SELECTOR,
            platform="darwin",
        )

        self.assertEqual(
            [gate.name for gate in gates],
            [
                "format",
                "lint-source-size",
                "lint-typescript",
                "lint-python",
                "lint-native",
                "lint-rust",
                "cli",
                "unit",
                "integration",
                "coverage-typescript",
                "coverage-native",
                "diff-check",
            ],
        )
        self.assertEqual(
            gates[0].argv,
            run_argv("format", "check-changed", "--base", "baseline-sha"),
        )
        # 每个语言分区都由 CLI 按 changed 范围认领属于自己的文件：显式文件集会因为差异里的
        # Markdown 之类没有适用工具的文件让整条命令拒绝执行（见 select_gates 注释）。
        for name in ("lint-typescript", "lint-python", "lint-native", "lint-rust"):
            with self.subTest(gate=name):
                gate = next(gate for gate in gates if gate.name == name)
                self.assertEqual(
                    gate.argv,
                    run_argv(
                        "lint",
                        "check-changed",
                        "--base",
                        "baseline-sha",
                        "--part",
                        name.removeprefix("lint-"),
                    ),
                )

    def test_select_gates_uses_quality_base_for_changed_scope(self) -> None:
        gates = select_gates(
            ["shell/packages/video-editor/src/index.ts"],
            base="baseline-sha",
            quality_base="merge-base-sha",
            package_index=_PACKAGE_INDEX,
            skill_test_files=_SKILL_TEST_FILES,
            coverage_selector=_COVERAGE_SELECTOR,
            platform="darwin",
        )

        # 三点 `{base}...HEAD` 与 CLI 的两点 changed 范围只有在 merge-base 上才等价。
        self.assertEqual(
            next(gate for gate in gates if gate.name == "format").argv,
            run_argv("format", "check-changed", "--base", "merge-base-sha"),
        )
        self.assertEqual(
            next(gate for gate in gates if gate.name == "diff-check").argv,
            ("git", "diff", "--check", "baseline-sha...HEAD"),
        )

    def test_select_gates_only_runs_native_coverage_on_darwin(self) -> None:
        changed_files = ["native/media/src/service.cpp"]

        darwin_names = [
            gate.name
            for gate in select_gates(
                changed_files,
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        ]
        windows_names = [
            gate.name
            for gate in select_gates(
                changed_files,
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="win32",
            )
        ]

        self.assertIn("coverage-native", darwin_names)
        self.assertNotIn("coverage-native", windows_names)

    def test_select_gates_only_serializes_resources_written_throughout_a_gate(self) -> None:
        darwin_gates = {
            gate.name: gate
            for gate in select_gates(
                ["native/media/src/service.cpp", "shell/apps/desktop/main.ts"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        }
        windows_gates = {
            gate.name: gate
            for gate in select_gates(
                ["native/media/src/service.cpp", "shell/apps/desktop/main.ts"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="win32",
            )
        }

        self.assertIn("native-build", darwin_gates["lint-native"].resources)
        self.assertEqual(darwin_gates["lint-python"].resources, frozenset())
        self.assertIn("desktop-pipeline", darwin_gates["integration"].resources)
        self.assertNotIn("native-build", darwin_gates["integration"].resources)
        self.assertNotIn("desktop-pipeline", windows_gates["integration"].resources)
        self.assertEqual(windows_gates["integration"].resources, frozenset())

    def test_select_gates_isolates_only_darwin_desktop_integration(self) -> None:
        other_package = WorkspacePackage(
            name="@libtv/other", root="shell/packages/other", layers=frozenset({"integ"})
        )
        package_index = PackageIndex((*_PACKAGE_INDEX.packages, other_package))
        cases = (
            ("darwin", "shell/vitest.unit.config.ts", "integration", True),
            ("darwin", "shell/apps/desktop/src/main.ts", "integration-typescript", True),
            ("darwin", "shell/packages/other/src/index.ts", "integration-typescript", False),
            ("darwin", "native/rust/crates/example/src/lib.rs", "integration-rust", False),
            ("win32", "shell/apps/desktop/src/main.ts", "integration-typescript", False),
            ("linux", "shell/apps/desktop/src/main.ts", "integration-typescript", False),
        )
        for platform, path, integration_name, expected in cases:
            with self.subTest(platform=platform, path=path):
                gates = select_gates(
                    [path],
                    base="baseline-sha",
                    package_index=package_index,
                    skill_test_files=_SKILL_TEST_FILES,
                    coverage_selector=_COVERAGE_SELECTOR,
                    platform=platform,
                )
                integration = next(gate for gate in gates if gate.name == integration_name)
                self.assertEqual(integration.requires_exclusive_execution, expected)
                self.assertTrue(
                    all(
                        not gate.requires_exclusive_execution
                        for gate in gates
                        if gate.name != integration_name
                    )
                )

    def test_select_gates_does_not_duplicate_cli_coverage(self) -> None:
        names = [
            gate.name
            for gate in select_gates(
                ["skills/gate-and-fix/scripts/gate_and_fix.py"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
            )
        ]

        self.assertIn("cli", names)
        self.assertNotIn("pr-script-tests", names)

    def test_select_gates_excludes_non_regular_paths_from_quality_commands(self) -> None:
        gates = select_gates(
            ["external/acp-ts"],
            base="baseline-sha",
            quality_files=[],
            package_index=_PACKAGE_INDEX,
            skill_test_files=_SKILL_TEST_FILES,
            coverage_selector=_COVERAGE_SELECTOR,
        )

        self.assertEqual([gate.name for gate in gates], ["diff-check"])

    def test_select_gates_rejects_non_repository_paths(self) -> None:
        with self.assertRaisesRegex(ValueError, "仓库相对路径"):
            select_gates(
                ["../outside.py"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
            )

    def test_select_gates_scopes_unit_tests_to_the_touched_packages(self) -> None:
        gates = {
            gate.name: gate
            for gate in select_gates(
                ["shell/packages/video-editor/core/src/timeline.ts"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        }

        self.assertEqual(
            gates["unit-typescript"].argv,
            run_argv(
                "test",
                "unit",
                "--part",
                "typescript",
                "--package",
                "@libtv/video-editor-core",
            ),
        )
        self.assertNotIn("unit", gates)
        self.assertNotIn("integration", gates)
        self.assertNotIn("integration-typescript", gates)
        self.assertNotIn("unit-rust", gates)

    def test_select_gates_runs_integration_only_for_a_package_declaring_it(self) -> None:
        gates = {
            gate.name: gate
            for gate in select_gates(
                [
                    "shell/apps/desktop/src/main.ts",
                    "shell/packages/video-editor/core/src/timeline.ts",
                ],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        }

        self.assertEqual(
            gates["unit-typescript"].argv,
            run_argv(
                "test",
                "unit",
                "--part",
                "typescript",
                "--package",
                "@liblib-ai/libtv-desktop",
                "--package",
                "@libtv/video-editor-core",
            ),
        )
        # video-editor-core 没有声明 test:integ，只有 desktop 进入 Integration 筛选。
        self.assertEqual(
            gates["integration-typescript"].argv,
            run_argv(
                "test",
                "integ",
                "--part",
                "typescript",
                "--package",
                "@liblib-ai/libtv-desktop",
            ),
        )
        self.assertEqual(
            gates["integration-typescript"].resources,
            frozenset({"desktop-pipeline"}),
        )

    def test_select_gates_keeps_rust_tests_at_language_scope(self) -> None:
        gates = {
            gate.name: gate
            for gate in select_gates(
                ["native/rust/sidecar/src/lib.rs"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        }

        # crate 是否有 unit test 无法从文件树判定，--package 会让无 unit test 的 crate
        # 以「筛选没有匹配到任何测试」假失败，因此 Rust 只做语言级收敛。
        self.assertEqual(
            gates["unit-rust"].argv, run_argv("test", "unit", "--part", "rust")
        )
        self.assertEqual(
            gates["integration-rust"].argv,
            run_argv("test", "integ", "--part", "rust"),
        )
        self.assertNotIn("unit-typescript", gates)
        self.assertNotIn("integration-typescript", gates)

    def test_select_gates_falls_back_to_full_layers_for_shared_product_inputs(
        self,
    ) -> None:
        for changed_file in (
            "shell/tsconfig.base.json",
            "shell/pnpm-lock.yaml",
            "shell/scripts/tool.ts",
            "native/media/src/service.cpp",
            "cmake/toolchain.cmake",
        ):
            with self.subTest(changed_file=changed_file):
                gates = {
                    gate.name: gate
                    for gate in select_gates(
                        [changed_file],
                        base="baseline-sha",
                        package_index=_PACKAGE_INDEX,
                        skill_test_files=_SKILL_TEST_FILES,
                        coverage_selector=_COVERAGE_SELECTOR,
                        platform="darwin",
                    )
                }

                self.assertEqual(gates["unit"].argv, run_argv("test", "unit"))
                self.assertEqual(gates["integration"].argv, run_argv("test", "integ"))
                self.assertNotIn("unit-typescript", gates)

    def test_select_gates_skips_tests_for_a_package_without_test_layers(self) -> None:
        names = [
            gate.name
            for gate in select_gates(
                ["shell/packages/artifact-verification/src/verify.ts"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        ]

        self.assertEqual(
            names,
            [
                "format",
                "lint-source-size",
                "lint-typescript",
                "lint-python",
                "lint-native",
                "lint-rust",
                "diff-check",
            ],
        )

    def test_select_test_scope_ignores_paths_outside_the_product_tree(self) -> None:
        scope = select_test_scope(
            ["docs/architecture/testing.md", "skills/gate-and-fix/SKILL.md"],
            package_index=_PACKAGE_INDEX,
        )

        self.assertFalse(scope.unscoped)
        self.assertFalse(scope.rust)
        self.assertEqual(scope.unit_packages, ())
        self.assertEqual(scope.integration_packages, ())

    def test_package_index_owner_prefers_the_deepest_package(self) -> None:
        index = PackageIndex(
            (
                WorkspacePackage(
                    name="outer", root="shell/packages", layers=frozenset({"unit"})
                ),
                WorkspacePackage(
                    name="inner",
                    root="shell/packages/video-editor/core",
                    layers=frozenset({"unit"}),
                ),
            )
        )

        owner = index.owner("shell/packages/video-editor/core/src/x.ts")

        self.assertIsNotNone(owner)
        self.assertEqual(owner.name, "inner")

    def test_bus_adapter_uses_ci_and_changed_skill_tests(self) -> None:
        gates = select_bus_gates(
            ["skills/pr/SKILL.md"],
            base="baseline-sha",
            skill_test_files=_SKILL_TEST_FILES,
            available_tools=frozenset({"just", "cargo-nextest"}),
        )

        by_name = {gate.name: gate for gate in gates}
        self.assertEqual(by_name["ci"].argv, ("just", "ci"))
        self.assertEqual(
            by_name["skill-tests"].argv,
            (
                sys.executable,
                "-m",
                "pytest",
                "-q",
                "skills/pr/scripts/test_pr_format_check.py",
            ),
        )
        self.assertEqual(
            by_name["diff-check"].argv,
            ("git", "diff", "--check", "baseline-sha...HEAD"),
        )

    def test_bus_adapter_has_executable_fallback_without_just_or_nextest(self) -> None:
        gates = select_bus_gates(
            ["src/main.rs"],
            base="baseline-sha",
            skill_test_files=_SKILL_TEST_FILES,
            available_tools=frozenset(),
        )

        self.assertEqual(
            [gate.name for gate in gates],
            [
                "format",
                "clippy",
                "test",
                "maintenance-test",
                "ui-hot-path-architecture-test",
                "integration-assets-test",
                "plugin-marketplace-install",
                "plugin-marketplace-test",
                "diff-check",
            ],
        )
        self.assertEqual(gates[0].argv, ("cargo", "fmt", "--check"))
        self.assertEqual(gates[2].argv, ("cargo", "test", "--locked"))
        self.assertEqual(
            gates[6].argv,
            (
                "bun",
                "--cwd=workers/plugin-marketplace",
                "install",
                "--frozen-lockfile",
            ),
        )

    def test_select_gates_skips_the_typescript_partition_for_an_inert_diff(self) -> None:
        for changed_file in (
            "skills/gate-and-fix/scripts/gate_and_fix.py",
            "docs/architecture/testing.md",
            "native/media/src/service.cpp",
            "native/rust/sidecar/src/lib.rs",
        ):
            with self.subTest(changed_file=changed_file):
                names = [
                    gate.name
                    for gate in select_gates(
                        [changed_file],
                        base="baseline-sha",
                        package_index=_PACKAGE_INDEX,
                        skill_test_files=_SKILL_TEST_FILES,
                        coverage_selector=_COVERAGE_SELECTOR,
                        platform="darwin",
                    )
                ]

                self.assertNotIn("lint-typescript", names)
                self.assertIn("lint-python", names)

    def test_select_gates_keeps_the_typescript_partition_for_an_unknown_suffix(
        self,
    ) -> None:
        # 判定保守：只要有一个文件不在确定无关的后缀表里就必须进 TypeScript 分区，
        # 否则 `.mjs`、`package.json`、`tsconfig.json` 这类输入会静默跳过整树 typecheck。
        for changed_file in (
            "scripts/lint/module-discovery.mjs",
            "shell/apps/desktop/package.json",
            "shell/tsconfig.base.json",
        ):
            with self.subTest(changed_file=changed_file):
                names = [
                    gate.name
                    for gate in select_gates(
                        [changed_file],
                        base="baseline-sha",
                        package_index=_PACKAGE_INDEX,
                        skill_test_files=_SKILL_TEST_FILES,
                        coverage_selector=_COVERAGE_SELECTOR,
                        platform="darwin",
                    )
                ]

                self.assertIn("lint-typescript", names)

    def test_select_cli_test_files_scopes_a_skill_change_to_its_own_tests(self) -> None:
        files = select_cli_test_files(
            ["skills/gate-and-fix/SKILL.md", "skills/gate-and-fix/scripts/x.py"],
            skill_test_files=_SKILL_TEST_FILES,
        )

        self.assertEqual(
            files, ["skills/gate-and-fix/scripts/test_gate_and_fix.py"]
        )

    def test_select_cli_test_files_runs_the_whole_lane_outside_skills(self) -> None:
        for changed_file in (
            "cli/commands/quality.py",
            "cli_extensions/example.py",
            "scripts/lint/module-discovery.mjs",
            # skills/ 顶层的跨 Skill 文档不属于任何单个 Skill。
            "skills/skill-architecture.md",
        ):
            with self.subTest(changed_file=changed_file):
                self.assertEqual(
                    select_cli_test_files(
                        [changed_file], skill_test_files=_SKILL_TEST_FILES
                    ),
                    [],
                )

    def test_select_cli_test_files_ignores_documentation_without_pytest(self) -> None:
        # docs/guides/** 没有任何 pytest 读它的内容，跑整条 lane 买不到一行覆盖。
        self.assertIsNone(
            select_cli_test_files(
                ["docs/guides/review-response-guide.md"],
                skill_test_files=_SKILL_TEST_FILES,
            )
        )
        # 与有测试树的 Skill 同时改动时，只留下该 Skill 自己的测试，不升级成整条 lane。
        self.assertEqual(
            select_cli_test_files(
                [
                    "docs/guides/review-response-guide.md",
                    "skills/gate-and-fix/SKILL.md",
                ],
                skill_test_files=_SKILL_TEST_FILES,
            ),
            ["skills/gate-and-fix/scripts/test_gate_and_fix.py"],
        )
        # 但 scripts/ 确实被十余个 cli/tests 用例引用，仍跑整条 lane。
        self.assertEqual(
            select_cli_test_files(
                ["docs/guides/example.md", "scripts/lint/module-discovery.mjs"],
                skill_test_files=_SKILL_TEST_FILES,
            ),
            [],
        )

    def test_select_cli_test_files_skips_a_skill_without_tests(self) -> None:
        # 有测试树的 Skill 才可筛选：pytest 对零匹配的显式筛选以退出码 2 结束，
        # 给一个没有测试的 Skill 发 --file 会造出假失败。
        self.assertIsNone(
            select_cli_test_files(
                ["skills/rebase-origin-main/SKILL.md"],
                skill_test_files=_SKILL_TEST_FILES,
            )
        )
        self.assertIsNone(
            select_cli_test_files(
                ["shell/apps/desktop/src/main.ts"],
                skill_test_files=_SKILL_TEST_FILES,
            )
        )

    def test_select_gates_passes_scoped_cli_test_files_to_the_runner(self) -> None:
        gates = {
            gate.name: gate
            for gate in select_gates(
                ["skills/gate-and-fix/scripts/gate_and_fix.py"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
                coverage_selector=_COVERAGE_SELECTOR,
                platform="darwin",
            )
        }

        self.assertEqual(
            gates["cli"].argv,
            run_argv(
                "test",
                "cli",
                "--file",
                "skills/gate-and-fix/scripts/test_gate_and_fix.py",
            ),
        )

    def test_skill_test_files_are_discovered_from_the_repository(self) -> None:
        owned = load_skill_test_files(Path(__file__).resolve().parents[3])

        self.assertEqual(
            owned["skills/gate-and-fix"],
            ("skills/gate-and-fix/scripts/test_gate_and_fix.py",),
        )
        # 没有 pytest 文件的 Skill 不进入筛选。
        self.assertNotIn("skills/rebase-origin-main", owned)

    def test_select_gates_skips_coverage_outside_the_measured_scope(self) -> None:
        for changed_file, expected in (
            ("shell/packages/video-editor/core/src/timeline.ts", True),
            ("shell/packages/canvas-platform/src/raw-input.ts", True),
            # .js 也在范围内：范围由 coverage 配置声明，不是按 TypeScript 后缀推断。
            ("shell/packages/chromium-frame-host/src/argv.js", True),
            ("shell/packages/artifact-verification/src/verify.ts", False),
            ("shell/apps/desktop/src/main/host/dev-case/runner.ts", False),
        ):
            with self.subTest(changed_file=changed_file):
                names = [
                    gate.name
                    for gate in select_gates(
                        [changed_file],
                        base="baseline-sha",
                        package_index=_PACKAGE_INDEX,
                        skill_test_files=_SKILL_TEST_FILES,
                        coverage_selector=_COVERAGE_SELECTOR,
                        platform="darwin",
                    )
                ]

                self.assertEqual("coverage-typescript" in names, expected)

    def test_bus_adapter_does_not_require_desktop_coverage_config(self) -> None:
        names = [
            gate.name
            for gate in select_bus_gates(
                ["src/main.rs"],
                base="baseline-sha",
                skill_test_files=_SKILL_TEST_FILES,
                available_tools=frozenset({"just", "cargo-nextest"}),
            )
        ]

        self.assertEqual(names, ["ci", "diff-check"])

    def test_select_gates_never_runs_the_bundle_build(self) -> None:
        # bundle build 约 2 分钟，是本地一轮最大的单项开销，而 Build Check 是 main 的
        # required status check，本地重跑买不到额外保证。
        for changed_file in (
            "shell/apps/desktop/electron-builder.config.cjs",
            "shell/packages/video-editor/core/src/timeline.ts",
            "native/media/src/service.cpp",
            ".run.yml",
            "external/acp-ts",
        ):
            with self.subTest(changed_file=changed_file):
                names = [
                    gate.name
                    for gate in select_gates(
                        [changed_file],
                        base="baseline-sha",
                        package_index=_PACKAGE_INDEX,
                        skill_test_files=_SKILL_TEST_FILES,
                        coverage_selector=_COVERAGE_SELECTOR,
                        platform="darwin",
                    )
                ]

                self.assertNotIn("build", names)

    def test_select_gates_requires_every_scope_index_from_the_caller(self) -> None:
        # 索引一律由调用方按已解析的 --repo 载入：从别的 checkout 调用时，靠 __file__
        # 反推仓库根会拿 A 的索引去筛 B 的差异。缺参数必须是 TypeError，不是静默回退。
        with self.assertRaises(TypeError):
            select_gates(
                ["shell/packages/video-editor/core/src/timeline.ts"],
                base="baseline-sha",
                package_index=_PACKAGE_INDEX,
                skill_test_files=_SKILL_TEST_FILES,
            )

    def test_skill_test_files_discovers_both_pytest_name_patterns(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            skill = repo / "skills" / "example" / "scripts"
            skill.mkdir(parents=True)
            (skill / "test_leading.py").write_text("", encoding="utf-8")
            (skill / "trailing_test.py").write_text("", encoding="utf-8")
            (skill / "helper.py").write_text("", encoding="utf-8")
            (repo / "skills" / "untested").mkdir()

            owned = load_skill_test_files(repo)

        self.assertEqual(
            owned["skills/example"],
            (
                "skills/example/scripts/test_leading.py",
                "skills/example/scripts/trailing_test.py",
            ),
        )
        self.assertNotIn("skills/untested", owned)

    def test_run_gates_starts_independent_gates_before_either_can_finish(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = (
                "from pathlib import Path\n"
                "import sys, time\n"
                "root = Path(sys.argv[1])\n"
                "mine = root / sys.argv[2]\n"
                "other = root / sys.argv[3]\n"
                "mine.touch()\n"
                "deadline = time.monotonic() + 2\n"
                "while not other.exists():\n"
                "    if time.monotonic() >= deadline:\n"
                "        raise SystemExit(9)\n"
                "    time.sleep(0.01)\n"
            )
            results = run_gates(
                [
                    Gate("first", (sys.executable, "-c", child, str(root), "first", "second")),
                    Gate("second", (sys.executable, "-c", child, str(root), "second", "first")),
                ],
                cwd=Path.cwd(),
            )

        self.assertEqual([result.exit_code for result in results], [0, 0])

    def test_run_gates_does_not_overlap_a_shared_writable_resource(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = (
                "from pathlib import Path\n"
                "import sys, time\n"
                "active = Path(sys.argv[1]) / 'active'\n"
                "if active.exists():\n"
                "    raise SystemExit(9)\n"
                "active.touch()\n"
                "try:\n"
                "    time.sleep(0.1)\n"
                "finally:\n"
                "    active.unlink()\n"
            )
            gates = [
                Gate(
                    "native-lint",
                    (sys.executable, "-c", child, str(root)),
                    frozenset({"native-build"}),
                ),
                Gate(
                    "native-coverage",
                    (sys.executable, "-c", child, str(root)),
                    frozenset({"native-build"}),
                ),
            ]
            results = run_gates(gates, cwd=Path.cwd())

        self.assertEqual([result.exit_code for result in results], [0, 0])

    def test_run_gates_isolates_measurement_before_between_and_after_ordinary_gates(self) -> None:
        for position in range(3):
            with self.subTest(position=position):
                gates = [Gate("unit", ("unit",)), Gate("coverage", ("coverage",))]
                gates.insert(
                    position,
                    Gate("measurement", ("measure",), requires_exclusive_execution=True),
                )
                submitted: dict[Future, Gate] = {}
                batches: list[set[str]] = []

                def submit(_run, gate, *, cwd):
                    future = Future()
                    submitted[future] = gate
                    return future

                def complete_one(active, *, return_when):
                    # 在这里控制完成顺序，使准入断言不依赖真实子进程是否在下一项启动前恰好结束。
                    batch = {submitted[future].name for future in active}
                    batches.append(batch)
                    if "measurement" in batch:
                        self.assertEqual(batch, {"measurement"})
                    future = next(iter(active))
                    gate = submitted[future]
                    future.set_result(GateResult(gate.name, gate.argv, 0, 0, "", ""))
                    return {future}, set(active) - {future}

                with (
                    patch("gate_and_fix.ThreadPoolExecutor") as executor,
                    patch("gate_and_fix.wait", side_effect=complete_one),
                ):
                    executor.return_value.__enter__.return_value.submit.side_effect = submit
                    results = run_gates(gates, cwd=Path.cwd())

                self.assertIn({"unit", "coverage"}, batches)
                self.assertEqual([result.name for result in results], [gate.name for gate in gates])
                self.assertTrue(all(result.passed for result in results))

    def test_run_gates_rejects_duplicate_names_before_launching_a_gate(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "launched"
            gate = Gate(
                "duplicate",
                (
                    sys.executable,
                    "-c",
                    "from pathlib import Path; Path(__import__('sys').argv[1]).touch()",
                    str(marker),
                ),
            )

            with self.assertRaisesRegex(ValueError, "名称重复"):
                run_gates([gate, gate], cwd=Path.cwd())

            self.assertFalse(marker.exists())

    def test_render_round_round_trips_complete_output_and_validates_it(self) -> None:
        samples = ("", "no final newline", "final newline\n", "trailing whitespace \t\n")
        for sample in samples:
            with self.subTest(sample=repr(sample)):
                artifact = render_round(
                    round_number=2,
                    base="base-sha",
                    head="head-sha",
                    changed_files=["shell/foo.ts"],
                    results=[
                        GateResult(
                            name="failure",
                            argv=("tool", "--arg"),
                            exit_code=1,
                            duration_ms=2,
                            stdout=sample,
                            stderr=sample,
                        )
                    ],
                )

                self.assertEqual(
                    self._decode_rendered_log(artifact, "stdout"), sample
                )
                self.assertEqual(
                    self._decode_rendered_log(artifact, "stderr"), sample
                )
                self.assertEqual(validate_round(artifact, expected_base="base-sha"), "FAIL")

    def test_render_round_accepts_a_negative_exit_code_as_a_failure(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["shell/foo.ts"],
            results=[
                GateResult(
                    name="interrupted",
                    argv=("tool",),
                    exit_code=-9,
                    duration_ms=1,
                    stdout="",
                    stderr="",
                )
            ],
        )

        self.assertIn("### interrupted — FAIL", artifact)
        self.assertIn("- Exit code: `-9`", artifact)
        self.assertEqual(validate_round(artifact, expected_base="base-sha"), "FAIL")

    def test_run_gates_records_a_launch_error_that_renders_and_validates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "does-not-exist"
            result = run_gates(
                [Gate("missing", (str(missing),))], cwd=Path.cwd()
            )[0]

        self.assertIsNone(result.exit_code)
        self.assertTrue(result.stderr.startswith("launch error: "))
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[result],
        )
        self.assertIn("### missing — FAIL", artifact)
        self.assertIn("- Exit code: `launch-error`", artifact)
        self.assertEqual(validate_round(artifact, expected_base="base-sha"), "FAIL")

    def test_main_writes_a_fresh_artifact_for_each_invocation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            run = repo / "run"
            run.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            run.chmod(0o755)
            (repo / "run.ps1").write_text("exit 0\n", encoding="utf-8")
            (repo / ".gitignore").write_text("artifacts/\n", encoding="utf-8")
            source = repo / "skills/gate-and-fix/scripts/example.py"
            source.parent.mkdir(parents=True)
            source.write_text("before\n", encoding="utf-8")
            # main 必须按 --repo 载入全部范围索引，因此临时仓库要自带它们的事实源。
            shell_root = repo / "shell"
            shell_root.mkdir()
            (shell_root / "pnpm-workspace.yaml").write_text(
                "packages:\n  - apps/*\n  - packages/*\n", encoding="utf-8"
            )
            (shell_root / "vitest.coverage.config.ts").write_text(
                "const VIDEO_EDITOR_SOURCES = [\n"
                "  'packages/video-editor/*/src/**/*.{ts,tsx}',\n"
                "]\n",
                encoding="utf-8",
            )
            self._git(repo, "init", "-q")
            self._git(repo, "config", "user.email", "test@example.com")
            self._git(repo, "config", "user.name", "Test User")
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-qm", "initial")
            base = self._git_output(repo, "rev-parse", "HEAD")
            source.write_text("after\n", encoding="utf-8")
            self._git(repo, "add", "skills/gate-and-fix/scripts/example.py")
            self._git(repo, "commit", "-qm", "change")

            artifact_root = repo / "artifacts"
            passing = [
                GateResult("ci", ("just", "ci"), 0, 1, "", ""),
                GateResult(
                    "skill-tests",
                    (sys.executable, "-m", "pytest", "-q", str(source.relative_to(repo))),
                    0,
                    1,
                    "",
                    "",
                ),
                GateResult(
                    "diff-check",
                    ("git", "diff", "--check", f"{base}...HEAD"),
                    0,
                    1,
                    "",
                    "",
                ),
            ]
            with patch("gate_and_fix.run_gates", return_value=passing):
                first = self._run_main(repo, base, artifact_root)
                second = self._run_main(repo, base, artifact_root)

            self.assertNotEqual(first, second)
            self.assertTrue(first.is_file())
            self.assertTrue(second.is_file())
            self.assertEqual(validate_round(first.read_text(encoding="utf-8"), expected_base=base), "PASS")
            self.assertEqual(validate_round(second.read_text(encoding="utf-8"), expected_base=base), "PASS")

    def test_main_rejects_a_dirty_worktree_without_an_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "run").write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            (repo / "run").chmod(0o755)
            (repo / "run.ps1").write_text("exit 0\n", encoding="utf-8")
            (repo / ".gitignore").write_text("artifacts/\n", encoding="utf-8")
            source = repo / "skills/gate-and-fix/scripts/example.py"
            source.parent.mkdir(parents=True)
            source.write_text("before\n", encoding="utf-8")
            # main 必须按 --repo 载入全部范围索引，因此临时仓库要自带它们的事实源。
            shell_root = repo / "shell"
            shell_root.mkdir()
            (shell_root / "pnpm-workspace.yaml").write_text(
                "packages:\n  - apps/*\n  - packages/*\n", encoding="utf-8"
            )
            (shell_root / "vitest.coverage.config.ts").write_text(
                "const VIDEO_EDITOR_SOURCES = [\n"
                "  'packages/video-editor/*/src/**/*.{ts,tsx}',\n"
                "]\n",
                encoding="utf-8",
            )
            self._git(repo, "init", "-q")
            self._git(repo, "config", "user.email", "test@example.com")
            self._git(repo, "config", "user.name", "Test User")
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-qm", "initial")
            base = self._git_output(repo, "rev-parse", "HEAD")
            source.write_text("after\n", encoding="utf-8")
            self._git(repo, "add", "skills/gate-and-fix/scripts/example.py")
            self._git(repo, "commit", "-qm", "change")
            source.write_text("dirty tracked change\n", encoding="utf-8")
            (repo / "untracked.txt").write_text("dirty\n", encoding="utf-8")

            stdout = io.StringIO()
            stderr = io.StringIO()
            artifact_root = repo / "artifacts"
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                exit_code = main(
                    [
                        "--repo",
                        str(repo),
                        "--base",
                        base,
                        "--round",
                        "1",
                        "--artifact-root",
                        str(artifact_root),
                    ]
                )

            self.assertEqual(exit_code, 2)
            self.assertEqual(stdout.getvalue(), "")
            self.assertIn("工作树", stderr.getvalue())
            self.assertFalse(artifact_root.exists())

    def test_main_reports_runner_errors_without_an_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            stdout = io.StringIO()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                exit_code = main(
                    [
                        "--repo",
                        directory,
                        "--base",
                        "missing",
                        "--round",
                        "1",
                        "--artifact-root",
                        f"{directory}/artifacts",
                    ]
                )

        self.assertEqual(exit_code, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("gate-and-fix runner failed", stderr.getvalue())

    def test_verify_reports_the_validated_outcome(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="cli",
                    argv=("./run", "test", "cli"),
                    exit_code=0,
                    duration_ms=1,
                    stdout="",
                    stderr="",
                )
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                self.assertEqual(
                    main(
                        [
                            "verify",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "base-sha",
                        ]
                    ),
                    0,
                )

        self.assertEqual(stdout.getvalue(), "PASS\n")

    def test_show_outputs_one_validated_log_stream_without_a_newline(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="lint",
                    argv=("./run", "lint", "check-files"),
                    exit_code=1,
                    duration_ms=1,
                    stdout="first line\nlast line",
                    stderr="error\n",
                )
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                self.assertEqual(
                    main(
                        [
                            "show",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "base-sha",
                            "--gate",
                            "lint",
                            "--stream",
                            "stdout",
                        ]
                    ),
                    0,
                )

        self.assertEqual(stdout.getvalue(), "first line\nlast line")

    def test_list_outputs_only_failed_gate_names(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="format",
                    argv=("./run", "format", "check-files"),
                    exit_code=0,
                    duration_ms=1,
                    stdout="",
                    stderr="",
                ),
                GateResult(
                    name="lint",
                    argv=("./run", "lint", "check-files"),
                    exit_code=1,
                    duration_ms=1,
                    stdout="failure",
                    stderr="",
                ),
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                self.assertEqual(
                    main(
                        [
                            "list",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "base-sha",
                        ]
                    ),
                    0,
                )

        self.assertEqual(stdout.getvalue(), "lint\n")

    def test_show_rejects_an_artifact_with_the_wrong_base(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="lint",
                    argv=("./run", "lint", "check-files"),
                    exit_code=1,
                    duration_ms=1,
                    stdout="failure",
                    stderr="",
                )
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                self.assertEqual(
                    main(
                        [
                            "show",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "other-base",
                            "--gate",
                            "lint",
                            "--stream",
                            "stdout",
                        ]
                    ),
                    2,
                )

        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("Base", stderr.getvalue())

    @staticmethod
    def _git(repo: Path, *arguments: str) -> None:
        subprocess.run(("git", *arguments), cwd=repo, check=True)

    @staticmethod
    def _git_output(repo: Path, *arguments: str) -> str:
        return subprocess.run(
            ("git", *arguments),
            cwd=repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def _run_main(self, repo: Path, base: str, artifact_root: Path) -> Path:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            self.assertEqual(
                main(
                    [
                        "--repo",
                        str(repo),
                        "--base",
                        base,
                        "--round",
                        "1",
                        "--artifact-root",
                        str(artifact_root),
                    ]
                ),
                0,
            )
        return Path(stdout.getvalue().strip())

    @staticmethod
    def _decode_rendered_log(artifact: str, title: str) -> str:
        lines = artifact.splitlines()
        index = lines.index(f"#### {title}")
        assert lines[index + 1] == ""
        assert lines[index + 2] == "- Encoding: `base64-utf8`"
        byte_count = int(lines[index + 3].removeprefix("- Bytes: `").removesuffix("`"))
        assert lines[index + 4] == ""
        assert lines[index + 5] == "```base64"
        raw = base64.b64decode(lines[index + 6], validate=True)
        assert len(raw) == byte_count
        assert lines[index + 7] == "```"
        return raw.decode("utf-8")


if __name__ == "__main__":
    unittest.main()
