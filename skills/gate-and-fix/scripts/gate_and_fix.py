#!/usr/bin/env python3
"""并行运行适用门禁，并生成可供修复循环消费的完整证据。"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


PRODUCT_PREFIXES = ("shell/", "native/", "cmake/")
WORKFLOW_PREFIXES = ("cli/", "cli_extensions/", "skills/", "scripts/", "docs/guides/")
NATIVE_SUFFIXES = {
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".cppm",
    ".ixx",
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".ipp",
    ".tpp",
    ".inl",
    ".m",
    ".mm",
}
SHELL_ROOT = "shell"
SKILLS_PREFIX = "skills/"
# workflow 前缀里没有任何 pytest 覆盖的那部分：`docs/guides/**` 是纯文档，cli/tests 下没有
# 一个用例读它的内容，为它跑整条 CLI lane 买不到一行覆盖。它仍受 format / lint / text
# hygiene 约束，只是不进 CLI 测试选择。`scripts/` 不在此列——十余个 cli/tests 用例确实
# 引用它。
CLI_TEST_EXEMPT_PREFIXES = ("docs/guides/",)
RUST_PREFIX = "native/rust/"
UNIT_LAYER = "unit"
INTEGRATION_LAYER = "integ"
_LAYER_SCRIPTS = {UNIT_LAYER: "test:unit", INTEGRATION_LAYER: "test:integ"}
# 与 TypeScript 类型输入确定无关的后缀。判定刻意保守：未知后缀一律算相关，多跑一次整树
# typecheck 只是慢，漏判会让类型错误在本地假绿通过。
TYPESCRIPT_INERT_SUFFIXES = frozenset({".py", ".rs", ".md"}) | NATIVE_SUFFIXES
MAX_PARALLEL_GATES = 4
DESKTOP_PIPELINE_RESOURCE = "desktop-pipeline"
_LOG_ENCODING = "base64-utf8"
_SCHEMA_VERSION = "2"


@dataclass(frozen=True)
class Gate:
    name: str
    argv: tuple[str, ...]
    resources: frozenset[str] = frozenset()
    requires_exclusive_execution: bool = False


@dataclass(frozen=True)
class GateResult:
    name: str
    argv: tuple[str, ...]
    exit_code: int | None
    duration_ms: int
    stdout: str
    stderr: str

    @property
    def passed(self) -> bool:
        return self.exit_code == 0


@dataclass(frozen=True)
class ArtifactGate:
    name: str
    passed: bool
    stdout: str
    stderr: str


@dataclass(frozen=True)
class PathSelector:
    """一组 exact path 与目录前缀；用于判断差异是否命中某个门禁自己声明的范围。"""

    exact_paths: frozenset[str]
    prefixes: tuple[str, ...]

    def matches(self, path: str) -> bool:
        return path in self.exact_paths or path.startswith(self.prefixes)


@dataclass(frozen=True)
class WorkspacePackage:
    """一个能被 `./run test --part typescript --package` 精确筛选的 Workspace 包。"""

    name: str
    root: str
    layers: frozenset[str]


@dataclass(frozen=True)
class PackageIndex:
    packages: tuple[WorkspacePackage, ...]

    def owner(self, path: str) -> WorkspacePackage | None:
        """返回拥有该路径的最深包；不属于任何包时返回 None。"""
        owner: WorkspacePackage | None = None
        for package in self.packages:
            if not path.startswith(f"{package.root}/"):
                continue
            if owner is None or len(package.root) > len(owner.root):
                owner = package
        return owner


@dataclass(frozen=True)
class SuiteScope:
    """从已提交差异导出的测试范围。"""

    unscoped: bool
    unit_packages: tuple[str, ...]
    integration_packages: tuple[str, ...]
    rust: bool


def run_argv(*arguments: str, os_name: str | None = None) -> tuple[str, ...]:
    """返回当前宿主可直接执行的统一工程入口 argv。"""
    if (os.name if os_name is None else os_name) == "nt":
        return (
            "powershell.exe",
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            "run.ps1",
            *arguments,
        )
    return ("./run", *arguments)


def _validate_paths(changed_files: Sequence[str]) -> tuple[str, ...]:
    changed = tuple(sorted(set(changed_files)))
    if not changed:
        raise ValueError("没有相对 base 的变更，无法建立 gate")
    if any(
        not path
        or Path(path).is_absolute()
        or any(part in {"", ".", ".."} for part in path.split("/"))
        for path in changed
    ):
        raise ValueError("changed 文件必须是规范的仓库相对路径")
    return changed


def _validate_paths_or_empty(paths: Sequence[str]) -> tuple[str, ...]:
    return _validate_paths(paths) if paths else ()


def _workspace_package_patterns(shell_root: Path) -> tuple[str, ...]:
    """读取 pnpm workspace 声明的包 glob；`../` 前缀的外部 Submodule 不是 gate 目标。"""
    manifest = shell_root / "pnpm-workspace.yaml"
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ValueError(f"无法读取 pnpm workspace manifest: {error}") from error

    patterns: list[str] = []
    inside = False
    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        item = re.fullmatch(r"( +)-\s+(.+?)\s*", line)
        if item is not None and inside:
            pattern = item.group(2).strip("'\"")
            if not pattern.startswith("../"):
                patterns.append(pattern)
            continue
        inside = re.fullmatch(r"packages:\s*(?:#.*)?", line) is not None
    if not patterns:
        raise ValueError("pnpm workspace manifest 缺少 packages 声明")
    return tuple(patterns)


def load_package_index(repo: Path) -> PackageIndex:
    """从 pnpm workspace 的真实 manifest 派生可按 `--package` 筛选的测试模块。

    只有自己声明了该层级 script 的包才可以进入筛选：CLI 对未声明层级的包直接报用法
    错误，对声明了但零匹配的筛选以退出码 2 结束，二者都会把 gate 变成假失败。
    """
    shell_root = repo / SHELL_ROOT
    packages: list[WorkspacePackage] = []
    seen: set[str] = set()
    for pattern in _workspace_package_patterns(shell_root):
        for manifest in sorted(shell_root.glob(f"{pattern}/package.json")):
            try:
                data = json.loads(manifest.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as error:
                raise ValueError(f"无法读取 Workspace package manifest: {manifest}: {error}") from error
            if not isinstance(data, dict):
                raise ValueError(f"package manifest 顶层必须是对象: {manifest}")
            name = data.get("name")
            if not isinstance(name, str) or not name:
                raise ValueError(f"package manifest 缺少有效 name: {manifest}")
            if name in seen:
                raise ValueError(f"Workspace package name 重复: {name}")
            seen.add(name)
            scripts = data.get("scripts", {})
            if not isinstance(scripts, dict):
                raise ValueError(f"package scripts 必须是对象: {manifest}")
            layers = frozenset(
                layer
                for layer, script in _LAYER_SCRIPTS.items()
                if isinstance(scripts.get(script), str) and scripts[script]
            )
            packages.append(
                WorkspacePackage(
                    name=name,
                    root=manifest.parent.relative_to(repo).as_posix(),
                    layers=layers,
                )
            )
    return PackageIndex(tuple(sorted(packages, key=lambda package: package.name)))


def select_test_scope(
    changed_files: Sequence[str], *, package_index: PackageIndex
) -> SuiteScope:
    """把已提交差异收敛为「只测被改到的模块」。

    Rust 只做语言级收敛：`--package` 对没有 unit test 的 crate 会以「筛选没有匹配到任何
    测试」失败（`libtv-file-type` 实测），而 crate 是否有 unit test 无法从文件树判定。
    """
    unit: set[str] = set()
    integration: set[str] = set()
    unscoped = False
    rust = False
    for path in changed_files:
        if not path.startswith(PRODUCT_PREFIXES):
            continue
        if path.startswith(RUST_PREFIX):
            rust = True
            continue
        owner = package_index.owner(path)
        if owner is None:
            # Workspace 清单、根 tsconfig、native C++ 与 cmake 都不属于任何可筛选包，
            # 影响面无法用 --package 表达，只能退回整层运行。
            unscoped = True
            continue
        if UNIT_LAYER in owner.layers:
            unit.add(owner.name)
        if INTEGRATION_LAYER in owner.layers:
            integration.add(owner.name)
    return SuiteScope(
        unscoped=unscoped,
        unit_packages=tuple(sorted(unit)),
        integration_packages=tuple(sorted(integration)),
        rust=rust,
    )


def _package_arguments(names: Sequence[str]) -> tuple[str, ...]:
    return tuple(argument for name in names for argument in ("--package", name))


def load_skill_test_files(repo: Path) -> dict[str, tuple[str, ...]]:
    """每个 `skills/<name>/` 自带的 pytest 文件；没有测试文件的 Skill 不进入筛选。"""
    skills_root = repo / "skills"
    owned: dict[str, tuple[str, ...]] = {}
    for entry in sorted(path for path in skills_root.iterdir() if path.is_dir()):
        # pytest 默认 `python_files` 收 `test_*.py` 与 `*_test.py` 两种；`cli/quality_files.py`
        # 的 `_is_test_source` 对 `skills/**` 也正是按这两种判定。只认一种会让另一种命名的
        # Skill 测试既不发 gate 也不被察觉——静默跳过比假失败更难发现。
        files = tuple(
            sorted(
                {
                    found.relative_to(repo).as_posix()
                    for pattern in ("test_*.py", "*_test.py")
                    for found in entry.rglob(pattern)
                }
            )
        )
        if files:
            owned[f"{SKILLS_PREFIX}{entry.name}"] = files
    return owned


def select_cli_test_files(
    changed_files: Sequence[str], *, skill_test_files: dict[str, tuple[str, ...]]
) -> list[str] | None:
    """把 workflow 改动收敛为 `./run test cli` 的筛选文件。

    返回 `None` 表示本轮不需要 CLI gate，空列表表示跑整条 lane。`skills/<name>/` 是自带
    测试树的独立 owner，可以只跑自己那棵；`cli/` 是单个 Python 包，`cli/tests` 一棵树覆盖
    全部命令，没有更细且可证明的归属，因此保持整条 lane；`CLI_TEST_EXEMPT_PREFIXES` 里的
    路径没有任何 pytest 覆盖，不贡献目标。
    """
    workflow = [
        path
        for path in changed_files
        if path.startswith(WORKFLOW_PREFIXES)
        and not path.startswith(CLI_TEST_EXEMPT_PREFIXES)
    ]
    if not workflow:
        return None
    files: set[str] = set()
    for path in workflow:
        if not path.startswith(SKILLS_PREFIX):
            return []
        parts = path.split("/")
        if len(parts) <= 2:
            # skills/ 顶层的跨 Skill 文档影响面不限于某一个 Skill。
            return []
        files.update(skill_test_files.get(f"{SKILLS_PREFIX}{parts[1]}", ()))
    return sorted(files) or None


def _touches_typescript_inputs(paths: Sequence[str]) -> bool:
    """只有全部改动文件都属于确定与 TypeScript 无关的后缀时才跳过 TypeScript 分区。"""
    return not all(Path(path).suffix in TYPESCRIPT_INERT_SUFFIXES for path in paths)


def load_coverage_selector(repo: Path) -> PathSelector:
    """从 coverage 配置自己声明的 source glob 读取 TypeScript 覆盖率门禁的范围。

    范围的唯一事实源是拥有它的那个配置文件，skill 只做派生，不复制一份 glob。
    glob 一律收敛成「首个通配符之前的目录前缀」，因此只会多判不会漏判——多判的代价是白跑
    一次覆盖率，漏判的代价是本地放行一个真的掉了覆盖的文件。
    """
    config = repo / SHELL_ROOT / "vitest.coverage.config.ts"
    try:
        text = config.read_text(encoding="utf-8")
    except OSError as error:
        raise ValueError(f"无法读取 TypeScript 覆盖率配置: {error}") from error

    match = re.search(
        r"^const VIDEO_EDITOR_SOURCES = \[\n(.*?)^\]", text, re.DOTALL | re.MULTILINE
    )
    if match is None:
        raise ValueError("覆盖率配置缺少 VIDEO_EDITOR_SOURCES 声明")

    exact_paths: set[str] = set()
    prefixes: set[str] = set()
    for line in match.group(1).splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("//"):
            continue
        quoted = re.fullmatch(r"'([^']+)',?", stripped)
        if quoted is None:
            raise ValueError(f"覆盖率配置含不受支持的 source 条目: {stripped}")
        glob = f"{SHELL_ROOT}/{quoted.group(1)}"
        wildcard = min(
            (index for index in (glob.find(token) for token in "*?[{") if index != -1),
            default=-1,
        )
        if wildcard == -1:
            exact_paths.add(glob)
            continue
        prefixes.add(glob[: glob.rfind("/", 0, wildcard) + 1])
    if not exact_paths and not prefixes:
        raise ValueError("覆盖率配置的 VIDEO_EDITOR_SOURCES 为空")
    return PathSelector(frozenset(exact_paths), tuple(sorted(prefixes)))


def select_gates(
    changed_files: Sequence[str],
    *,
    base: str,
    quality_base: str | None = None,
    quality_files: Sequence[str] | None = None,
    coverage_selector: PathSelector,
    package_index: PackageIndex,
    skill_test_files: dict[str, tuple[str, ...]],
    platform: str | None = None,
) -> list[Gate]:
    """从不可变提交差异导出完整 gate 集合。

    三个范围索引都由调用方按已解析的 `--repo` 载入后传入，不在这里从 `__file__` 反推仓库根：
    从别的 checkout 调用本脚本时，那样会拿 A 的包索引去筛 B 的差异，发出指向不存在文件的
    `--file`，或者漏筛真正被改的包。

    `quality_base` 是 quality gate 用的 changed 范围 base，必须是 `base` 与 HEAD 的 merge-base：
    CLI 的 changed 范围是两点 `git diff <base>`，而本脚本其余地方一律用三点 `<base>...HEAD`。
    `base` 不是 HEAD 祖先时（origin/main 在本轮开始后又前进），两者会差出「main 独有提交」那部分
    文件，把与本差异无关的文件送进 lint。缺省回落 `base`，由调用方保证二者一致。
    """
    changed = _validate_paths(changed_files)
    quality_scope = changed if quality_files is None else _validate_paths_or_empty(quality_files)
    quality_diff_base = base if quality_base is None else quality_base
    coverage = coverage_selector
    index = package_index
    skill_tests = skill_test_files
    scope = select_test_scope(changed, package_index=index)
    touches_native = any(Path(path).suffix in NATIVE_SUFFIXES for path in changed)
    resolved_platform = sys.platform if platform is None else platform

    gates: list[Gate] = []
    if quality_scope:
        # 每个分区都由 CLI 自己认领属于该工具的文件——skill 不重建文件到语言的分类，否则
        # `.mjs` 这类 ESLint 认领而后缀表没有的文件会被静默漏掉。唯一由本 skill 决定的是
        # 「要不要进 TypeScript 分区」，因为 part=None 会无条件走整树 typecheck 分支。
        #
        # 用 changed 范围而不是把差异文件逐个作为显式参数：`check-files` 要求**显式文件必须
        # 全部被本次动作认领**，任何一个没有适用工具的文件（本仓差异里最常见的是 `AGENTS.md`
        # 等 Markdown）都会让整条命令以 UsageError 退出且**一个工具都不执行**
        # （`cli/commands/quality.py` 的「本次动作不能处理全部显式文件」），于是每个分区都假失败。
        # changed 范围按设计允许过滤掉没有工具认领的文件，与这里「让 CLI 自己认领」的意图一致。
        changed_scope = ("--base", quality_diff_base)
        gates.append(Gate("format", run_argv("format", "check-changed", *changed_scope)))
        gates.append(
            Gate(
                "lint-source-size",
                run_argv("lint", "check-changed", *changed_scope, "--part", "source-size"),
            )
        )
        if _touches_typescript_inputs(quality_scope):
            gates.append(
                Gate(
                    "lint-typescript",
                    run_argv(
                        "lint", "check-changed", *changed_scope, "--part", "typescript"
                    ),
                )
            )
        gates.append(
            Gate(
                "lint-python",
                run_argv("lint", "check-changed", *changed_scope, "--part", "python"),
            )
        )
        gates.append(
            Gate(
                "lint-native",
                run_argv("lint", "check-changed", *changed_scope, "--part", "native"),
                frozenset({"native-build"}) if touches_native else frozenset(),
            )
        )
        gates.append(
            Gate(
                "lint-rust",
                run_argv("lint", "check-changed", *changed_scope, "--part", "rust"),
            )
        )
    cli_test_files = select_cli_test_files(changed, skill_test_files=skill_tests)
    if cli_test_files is not None:
        gates.append(
            Gate(
                "cli",
                run_argv(
                    "test",
                    "cli",
                    *(
                        argument
                        for file in cli_test_files
                        for argument in ("--file", file)
                    ),
                ),
            )
        )
    integration_resources = (
        frozenset({DESKTOP_PIPELINE_RESOURCE})
        if resolved_platform == "darwin"
        else frozenset()
    )
    # Desktop 的 Integration 入口包含墙钟性能判据；独立 Vitest 进程不能隔离其他 gate 的负载。
    integration_requires_exclusive_execution = resolved_platform == "darwin" and (
        scope.unscoped
        or any(
            package.root == "shell/apps/desktop" and package.name in scope.integration_packages
            for package in index.packages
        )
    )
    if scope.unscoped:
        gates.extend(
            [
                Gate("unit", run_argv("test", "unit")),
                Gate(
                    "integration",
                    run_argv("test", "integ"),
                    integration_resources,
                    requires_exclusive_execution=integration_requires_exclusive_execution,
                ),
            ]
        )
    else:
        if scope.unit_packages:
            gates.append(
                Gate(
                    "unit-typescript",
                    run_argv(
                        "test",
                        "unit",
                        "--part",
                        "typescript",
                        *_package_arguments(scope.unit_packages),
                    ),
                )
            )
        if scope.rust:
            gates.append(
                Gate("unit-rust", run_argv("test", "unit", "--part", "rust"))
            )
        if scope.integration_packages:
            gates.append(
                Gate(
                    "integration-typescript",
                    run_argv(
                        "test",
                        "integ",
                        "--part",
                        "typescript",
                        *_package_arguments(scope.integration_packages),
                    ),
                    integration_resources,
                    requires_exclusive_execution=integration_requires_exclusive_execution,
                )
            )
        if scope.rust:
            gates.append(
                Gate("integration-rust", run_argv("test", "integ", "--part", "rust"))
            )
    # CLI 以同一 base 收敛到变更生产文件；跨包测试归属仍由 Vitest 依赖图决定。
    if any(coverage.matches(path) for path in changed):
        gates.append(
            Gate("coverage-typescript", run_argv("test", "coverage", "typescript", "--base", base))
        )
    if touches_native and resolved_platform == "darwin":
        gates.append(
            Gate(
                "coverage-native",
                run_argv("test", "coverage", "native", "--base", base),
                frozenset({"native-build"}),
            )
        )
    gates.append(Gate("diff-check", ("git", "diff", "--check", f"{base}...HEAD")))
    return gates


def select_bus_gates(
    changed_files: Sequence[str],
    *,
    base: str,
    skill_test_files: dict[str, tuple[str, ...]],
    available_tools: frozenset[str] | None = None,
) -> list[Gate]:
    """Select Bus gates while retaining the canonical artifact protocol.

    Bus has one Rust workspace and no Desktop pnpm/coverage partitions. `just
    ci` is the repository's complete Unix pre-PR gate. Workflow Python tests
    are added when the diff touches a skill that owns tests because `just ci`
    intentionally does not discover arbitrary skill-local pytest files.
    """

    changed = _validate_paths(changed_files)
    tools = available_tools
    if tools is None:
        tools = frozenset(
            name for name in ("just", "cargo-nextest") if shutil.which(name)
        )
    if {"just", "cargo-nextest"}.issubset(tools):
        gates = [Gate("ci", ("just", "ci"), requires_exclusive_execution=True)]
    else:
        maintenance_tests = (
            "scripts.test_agent_detection_manifest_check",
            "scripts.test_bus_dev_acceptance",
            "scripts.test_changelog",
            "scripts.test_config_reference_check",
            "scripts.test_docs_translation_parity",
            "scripts.test_hermes_integration_asset",
            "scripts.test_package_windows_conpty",
            "scripts.test_preview",
            "scripts.test_sanitize_review_severity",
            "scripts.test_skill_migration_contract",
            "scripts.test_unix_installer",
            "scripts.test_vendor_libghostty_vt",
            "scripts.test_vendor_portable_pty",
        )
        gates = [
            Gate(
                "format",
                ("cargo", "fmt", "--check"),
                requires_exclusive_execution=True,
            ),
            Gate(
                "clippy",
                (
                    "cargo",
                    "clippy",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                requires_exclusive_execution=True,
            ),
            Gate(
                "test",
                ("cargo", "test", "--locked"),
                requires_exclusive_execution=True,
            ),
            Gate(
                "maintenance-test",
                (sys.executable, "-m", "unittest", *maintenance_tests),
                requires_exclusive_execution=True,
            ),
            Gate(
                "ui-hot-path-architecture-test",
                (
                    sys.executable,
                    "-m",
                    "unittest",
                    "scripts.test_ui_hot_path_architecture",
                ),
                requires_exclusive_execution=True,
            ),
            Gate(
                "integration-assets-test",
                (
                    "bun",
                    "test",
                    "src/integration/assets/herdr-agent-state.test.ts",
                    "src/integration/assets/opencode/herdr-agent-state.test.ts",
                    "src/integration/assets/opencode/herdr-tui-session.test.ts",
                ),
                requires_exclusive_execution=True,
            ),
            Gate(
                "plugin-marketplace-install",
                (
                    "bun",
                    "--cwd=workers/plugin-marketplace",
                    "install",
                    "--frozen-lockfile",
                ),
                requires_exclusive_execution=True,
            ),
            Gate(
                "plugin-marketplace-test",
                ("bun", "--cwd=workers/plugin-marketplace", "test"),
                requires_exclusive_execution=True,
            ),
        ]
    selected_tests: set[str] = set()
    for path in changed:
        parts = path.split("/")
        if len(parts) > 2 and parts[0] == "skills":
            selected_tests.update(skill_test_files.get(f"skills/{parts[1]}", ()))
        elif path.startswith(("cli_extensions/", "docs/guides/", "docs/templates/")):
            selected_tests.update(
                test
                for tests in skill_test_files.values()
                for test in tests
            )
    if selected_tests:
        gates.append(
            Gate(
                "skill-tests",
                (sys.executable, "-m", "pytest", "-q", *sorted(selected_tests)),
            )
        )
    gates.append(Gate("diff-check", ("git", "diff", "--check", f"{base}...HEAD")))
    return gates


def _run_gate(gate: Gate, *, cwd: Path) -> GateResult:
    started = time.monotonic()
    try:
        process = subprocess.Popen(
            gate.argv,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as error:
        return GateResult(
            name=gate.name,
            argv=gate.argv,
            exit_code=None,
            duration_ms=round((time.monotonic() - started) * 1000),
            stdout="",
            stderr=f"launch error: {error}",
        )

    stdout, stderr = process.communicate()
    return GateResult(
        name=gate.name,
        argv=gate.argv,
        exit_code=process.returncode,
        duration_ms=round((time.monotonic() - started) * 1000),
        stdout=stdout,
        stderr=stderr,
    )


def run_gates(gates: Sequence[Gate], *, cwd: Path) -> list[GateResult]:
    """并行独立 gate，互斥可写资源，并隔离本轮独占执行的性能测量。"""
    if not gates:
        raise ValueError("至少需要一个 gate")
    if len({gate.name for gate in gates}) != len(gates):
        raise ValueError("gate 名称重复")

    pending = list(gates)
    active_resources: set[str] = set()
    results: dict[str, GateResult] = {}
    with ThreadPoolExecutor(
        max_workers=min(MAX_PARALLEL_GATES, len(gates)), thread_name_prefix="gate-and-fix"
    ) as executor:
        active = {}
        while pending or active:
            while len(active) < MAX_PARALLEL_GATES:
                if any(gate.requires_exclusive_execution for gate in active.values()):
                    break
                index = next(
                    (
                        candidate_index
                        for candidate_index, candidate in enumerate(pending)
                        if not active_resources.intersection(candidate.resources)
                        and not (candidate.requires_exclusive_execution and active)
                    ),
                    None,
                )
                if index is None:
                    break
                gate = pending.pop(index)
                active_resources.update(gate.resources)
                active[executor.submit(_run_gate, gate, cwd=cwd)] = gate
            if not active:
                raise ValueError("gate 资源调度无法推进")
            completed, _ = wait(active, return_when=FIRST_COMPLETED)
            for future in completed:
                gate = active.pop(future)
                active_resources.difference_update(gate.resources)
                results[gate.name] = future.result()
    return [results[gate.name] for gate in gates]


def _render_log(title: str, value: str) -> list[str]:
    encoded = base64.b64encode(value.encode("utf-8")).decode("ascii")
    return [
        f"#### {title}",
        "",
        f"- Encoding: `{_LOG_ENCODING}`",
        f"- Bytes: `{len(value.encode('utf-8'))}`",
        "",
        "```base64",
        encoded,
        "```",
        "",
    ]


def render_round(
    *,
    round_number: int,
    base: str,
    head: str,
    changed_files: Sequence[str],
    results: Sequence[GateResult],
) -> str:
    """生成格式 SOT 定义的完整 Markdown 证据。"""
    outcome = "PASS" if all(result.passed for result in results) else "FAIL"
    lines = [
        "# Gate-and-Fix Round",
        "",
        f"- Schema: `{_SCHEMA_VERSION}`",
        f"- Round: `{round_number}`",
        f"- Base: `{base}`",
        f"- Head: `{head}`",
        f"- Outcome: `{outcome}`",
        "",
        "## Changed files",
        "",
        *[f"- `{path}`" for path in sorted(set(changed_files))],
        "",
        "## Gate results",
        "",
    ]
    for result in results:
        status = "PASS" if result.passed else "FAIL"
        exit_code = str(result.exit_code) if result.exit_code is not None else "launch-error"
        lines.extend(
            [
                f"### {result.name} — {status}",
                "",
                f"- Command: `{shlex.join(result.argv)}`",
                f"- Exit code: `{exit_code}`",
                f"- Duration: `{result.duration_ms}ms`",
                "",
                *_render_log("stdout", result.stdout),
                *_render_log("stderr", result.stderr),
            ]
        )
    return "\n".join(lines)


def _expect(lines: list[str], index: int, expected: str) -> int:
    if index >= len(lines) or lines[index] != expected:
        actual = "<eof>" if index >= len(lines) else repr(lines[index])
        raise ValueError(f"artifact 结构错误：期望 {expected!r}，实际 {actual}")
    return index + 1


def _parse_log(
    lines: list[str], index: int, title: str, *, allow_eof: bool = False
) -> tuple[int, str]:
    index = _expect(lines, index, f"#### {title}")
    index = _expect(lines, index, "")
    index = _expect(lines, index, f"- Encoding: `{_LOG_ENCODING}`")
    if index >= len(lines) or not re.fullmatch(r"- Bytes: `[0-9]+`", lines[index]):
        raise ValueError("artifact 日志 Bytes 非法")
    byte_count = int(lines[index].removeprefix("- Bytes: `").removesuffix("`"))
    index += 1
    index = _expect(lines, index, "")
    index = _expect(lines, index, "```base64")
    if index >= len(lines):
        raise ValueError("artifact 日志缺少 base64 内容")
    encoded = lines[index]
    index += 1
    index = _expect(lines, index, "```")
    try:
        value = base64.b64decode(encoded, validate=True)
    except ValueError as error:
        raise ValueError("artifact 日志 base64 非法") from error
    if len(value) != byte_count:
        raise ValueError("artifact 日志 Bytes 与内容不一致")
    try:
        decoded = value.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("artifact 日志不是 UTF-8") from error
    if allow_eof and index == len(lines):
        return index, decoded
    return _expect(lines, index, ""), decoded


def _parse_round(
    artifact: str, *, expected_base: str
) -> tuple[str, dict[str, ArtifactGate]]:
    """fail-closed 解析 artifact，并仅在完整校验后返回各 gate 日志。"""
    lines = artifact.splitlines()
    index = 0
    index = _expect(lines, index, "# Gate-and-Fix Round")
    index = _expect(lines, index, "")
    index = _expect(lines, index, f"- Schema: `{_SCHEMA_VERSION}`")
    if index >= len(lines) or not re.fullmatch(r"- Round: `[1-9][0-9]*`", lines[index]):
        raise ValueError("artifact Round 非法")
    index += 1
    index = _expect(lines, index, f"- Base: `{expected_base}`")
    if index >= len(lines) or not re.fullmatch(r"- Head: `[^`]+`", lines[index]):
        raise ValueError("artifact Head 非法")
    index += 1
    if index >= len(lines) or not re.fullmatch(r"- Outcome: `(PASS|FAIL)`", lines[index]):
        raise ValueError("artifact Outcome 非法")
    outcome = lines[index].removeprefix("- Outcome: `").removesuffix("`")
    index += 1
    index = _expect(lines, index, "")
    index = _expect(lines, index, "## Changed files")
    index = _expect(lines, index, "")
    changed_files = []
    while index < len(lines) and lines[index].startswith("- `"):
        match = re.fullmatch(r"- `([^`]+)`", lines[index])
        if match is None:
            raise ValueError("artifact Changed files 条目非法")
        changed_files.append(match.group(1))
        index += 1
    if not changed_files or changed_files != sorted(set(changed_files)):
        raise ValueError("artifact Changed files 必须非空、排序且去重")
    index = _expect(lines, index, "")
    index = _expect(lines, index, "## Gate results")
    index = _expect(lines, index, "")
    statuses: list[bool] = []
    gates: dict[str, ArtifactGate] = {}
    while index < len(lines):
        match = re.fullmatch(r"### ([a-z0-9-]+) — (PASS|FAIL)", lines[index])
        if match is None:
            raise ValueError("artifact Gate result 标题非法")
        gate_name = match.group(1)
        if gate_name in gates:
            raise ValueError("artifact gate 名称重复")
        status = match.group(2)
        index += 1
        index = _expect(lines, index, "")
        if index >= len(lines) or not re.fullmatch(r"- Command: `[^`]+`", lines[index]):
            raise ValueError("artifact Command 非法")
        index += 1
        if index >= len(lines) or not re.fullmatch(r"- Exit code: `(?:-?[0-9]+|launch-error)`", lines[index]):
            raise ValueError("artifact Exit code 非法")
        exit_code = lines[index].removeprefix("- Exit code: `").removesuffix("`")
        passed = exit_code != "launch-error" and int(exit_code) == 0
        if (status == "PASS") != passed:
            raise ValueError("artifact gate 状态与退出码不一致")
        statuses.append(passed)
        index += 1
        if index >= len(lines) or not re.fullmatch(r"- Duration: `[0-9]+ms`", lines[index]):
            raise ValueError("artifact Duration 非法")
        index += 1
        index = _expect(lines, index, "")
        index, stdout = _parse_log(lines, index, "stdout")
        index, stderr = _parse_log(lines, index, "stderr", allow_eof=True)
        gates[gate_name] = ArtifactGate(
            name=gate_name,
            passed=status == "PASS",
            stdout=stdout,
            stderr=stderr,
        )
    if not statuses:
        raise ValueError("artifact 至少需要一个 gate result")
    derived_outcome = "PASS" if all(statuses) else "FAIL"
    if outcome != derived_outcome:
        raise ValueError("artifact Outcome 与 gate 结果不一致")
    return outcome, gates


def validate_round(artifact: str, *, expected_base: str) -> str:
    """fail-closed 验证 artifact 结构、结果枚举与汇总 outcome。"""
    return _parse_round(artifact, expected_base=expected_base)[0]


def _git_output(repo: Path, *arguments: str) -> str:
    result = subprocess.run(
        ("git", *arguments),
        cwd=repo,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"git {' '.join(arguments)} failed: {detail}")
    return result.stdout.strip()


def _write_artifact(root: Path, *, round_number: int, artifact: str) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    path = root / f"round-{round_number}-{uuid.uuid4().hex}.md"
    with path.open("x", encoding="utf-8") as output:
        output.write(artifact)
    return path


def _verify_main(argv: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(description="验证 gate-and-fix Markdown round artifact")
    parser.add_argument("--artifact", required=True)
    parser.add_argument("--base", required=True)
    args = parser.parse_args(argv)
    try:
        outcome = validate_round(
            Path(args.artifact).read_text(encoding="utf-8"),
            expected_base=args.base,
        )
    except (OSError, ValueError) as error:
        print(f"gate-and-fix verifier failed: {error}", file=sys.stderr)
        return 2
    print(outcome)
    return 0


def _show_main(argv: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(description="输出已校验 gate artifact 的单一完整日志流")
    parser.add_argument("--artifact", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--gate", required=True)
    parser.add_argument("--stream", required=True, choices=("stdout", "stderr"))
    args = parser.parse_args(argv)
    try:
        _, gates = _parse_round(
            Path(args.artifact).read_text(encoding="utf-8"),
            expected_base=args.base,
        )
        gate = gates.get(args.gate)
        if gate is None:
            raise ValueError(f"artifact 不包含 gate: {args.gate}")
    except (OSError, ValueError) as error:
        print(f"gate-and-fix show failed: {error}", file=sys.stderr)
        return 2
    sys.stdout.write(gate.stdout if args.stream == "stdout" else gate.stderr)
    return 0


def _list_main(argv: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(description="列出已校验 gate artifact 的失败 gate 名称")
    parser.add_argument("--artifact", required=True)
    parser.add_argument("--base", required=True)
    args = parser.parse_args(argv)
    try:
        _, gates = _parse_round(
            Path(args.artifact).read_text(encoding="utf-8"),
            expected_base=args.base,
        )
    except (OSError, ValueError) as error:
        print(f"gate-and-fix list failed: {error}", file=sys.stderr)
        return 2
    for gate in gates.values():
        if not gate.passed:
            print(gate.name)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    arguments = tuple(sys.argv[1:] if argv is None else argv)
    if arguments[:1] == ("verify",):
        return _verify_main(arguments[1:])
    if arguments[:1] == ("list",):
        return _list_main(arguments[1:])
    if arguments[:1] == ("show",):
        return _show_main(arguments[1:])
    parser = argparse.ArgumentParser(
        description="并行运行适用 gate，并写出唯一的 Markdown round artifact"
    )
    parser.add_argument("--repo", default=".")
    parser.add_argument("--base", required=True, help="rebase 后不可变 baseline commit")
    parser.add_argument("--round", required=True, type=int)
    parser.add_argument("--artifact-root", required=True, help="本次调用专用的 artifact 根目录")
    args = parser.parse_args(arguments)

    try:
        if args.round < 1:
            raise ValueError("round 必须大于 0")
        repo = Path(args.repo).resolve()
        if _git_output(repo, "status", "--porcelain", "--untracked-files=all"):
            raise ValueError(
                "工作树不干净：请提交或移除全部已跟踪和未跟踪的修改后再运行 gate-and-fix"
            )
        base = _git_output(repo, "rev-parse", "--verify", f"{args.base}^{{commit}}")
        head = _git_output(repo, "rev-parse", "HEAD")
        changed_files = _git_output(repo, "diff", "--name-only", f"{base}...HEAD").splitlines()
        quality_files = [
            path for path in changed_files if (repo / path).is_file()
        ]
        gates = select_bus_gates(
            changed_files,
            base=base,
            skill_test_files=load_skill_test_files(repo),
        )
        results = run_gates(gates, cwd=repo)
        artifact_path = _write_artifact(
            Path(args.artifact_root).resolve(),
            round_number=args.round,
            artifact=render_round(
                round_number=args.round,
                base=base,
                head=head,
                changed_files=changed_files,
                results=results,
            ),
        )
        outcome = validate_round(
            artifact_path.read_text(encoding="utf-8"), expected_base=base
        )
    except (OSError, ValueError) as error:
        print(f"gate-and-fix runner failed: {error}", file=sys.stderr)
        return 2

    print(artifact_path)
    return 0 if outcome == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
