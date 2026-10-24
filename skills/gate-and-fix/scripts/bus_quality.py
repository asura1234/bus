#!/usr/bin/env python3
"""The same four mandatory checks for just and the gate runner, without live agents/UI."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

from coverage_run import EXTERNAL_RUN, PROFILE_DIR, Run
from gate_process import Children, Interrupted, interrupt_signals
from profile_report import merged_profile

ROOT = Path(__file__).resolve().parents[3]
OUTPUT = ROOT / "temp/gate-and-fix/coverage"
POLICY = ROOT / "skills/gate-and-fix/references/coverage-policy.json"
LINT_POLICY = ROOT / "skills/gate-and-fix/references/lint-policy.toml"
LANES = ("lint", "unit", "integration", "coverage")
PYTHON_ROOTS = ("scripts", "skills", "cli_extensions", "tools", "packaging")
RUST_ROOTS = ("src", "tests")
RUST_SOURCE_ROOT = "src"
_INITIAL_LINT_POLICY = tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))
GENERATED_RUST = tuple(_INITIAL_LINT_POLICY["generated_files"])
RUST_EXCLUDE = "|".join(
    (
        r"/(tests|vendor)/",
        *((r"/(tests|test_support)\.rs$",)
          if "test_files" not in _INITIAL_LINT_POLICY else ()),
        r"/build\.rs$",
        *("/" + re.escape(path) + "$" for path in GENERATED_RUST),
    )
)
IN_PROCESS_SERVER_TESTS = "server::tests::"
# The dev variant (`./run dev`) compiles test tooling behind this feature; lint
# it with the same policy and run the CLI tests that reach it.
DEV_TOOLS_FEATURES = ("--features", "dev-tools")
DEV_TOOLS_TEST_TARGETS = ("cli", "tui-driver")
# The driver's own unit tests live in the bus binary under this module path.
DEV_TOOLS_UNIT_PREFIX = "devtools::"
# The default rustc wrapper injects flags outside Cargo's cache key. Normal and
# coverage builds may share one target, so instrumentation must use RUSTFLAGS.
COVERAGE_BUILD_FLAGS = ("--no-rustc-wrapper",)
ARCHITECTURE_TESTS = (
    "tools/tests/ui_hot_path_test.py",
    "tools/tests/import_boundaries_test.py",
    "tools/tests/scopes_test.py",
    "tools/tests/test_placement_check_test.py",
)
IMPORT_BOUNDARIES = "tools.quality.import_boundaries"
# Preserve the existing coverage scope rules until the S10 policy activation.
LEGACY_TEST_MODULE = re.compile(
    r"(?m)^[ \t]*#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*"
    r"(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{"
)


def python_roots() -> tuple[str, ...]:
    return tuple(name for name in PYTHON_ROOTS if (ROOT / name).is_dir())


def child_env(**overrides: str) -> dict:
    """A gate started from a Bus agent pane inherits that live Bus's BUS_*/HERDR_*
    variables; tests must never see them (or reach its data dir), so drop them all."""
    env = {
        name: value for name, value in os.environ.items()
        if not name.startswith(("BUS_", "HERDR_"))
    }
    return {**env, **overrides}


def run(*argv: str, env: dict | None = None) -> int:
    print("+ " + subprocess.list2cmdline(argv), flush=True)
    children = Children(private_groups=not bool(os.environ.get(EXTERNAL_RUN)))
    process = children.start(argv, cwd=ROOT, env=child_env() if env is None else env)
    try:
        process.wait()
        return process.returncode
    except BaseException:
        children.stop()
        raise
    finally:
        children.finished(process)


def python_files() -> list[Path]:
    return sorted(path for name in python_roots() for path in (ROOT / name).rglob("*.py"))


def is_test(path: Path) -> bool:
    return path.name.endswith("_test.py")


def configured_test_scopes(policy: dict) -> dict | None:
    """Final S10b keys activate consumers together; S10a retains legacy behavior."""
    keys = ("test_files", "test_dirs")
    if not any(key in policy for key in keys):
        return None
    if not all(key in policy for key in keys):
        raise ValueError("test_files and test_dirs must activate together")
    if any(not isinstance(policy[key], list) or
           any(not isinstance(value, str) or not value for value in policy[key])
           for key in keys):
        raise ValueError("invalid test scope patterns")
    return {key: tuple(policy[key]) for key in keys}


def clippy_commands(
    policy: dict, target: str | None = None, features: tuple[str, ...] = ()
) -> tuple[tuple[str, ...], ...]:
    """Check production before allowing selected lints while compiling tests."""
    base = ("cargo", "clippy", *features, *(("--target", target) if target else ()))
    all_targets = (*base, "--all-targets", "--locked", "--", "-D", "warnings")
    scopes = configured_test_scopes(policy)
    selected = policy.get("production_clippy_lints", [])
    if not isinstance(selected, list) or any(
        not isinstance(lint, str) or not re.fullmatch(r"(?:clippy::)?[a-z_]+", lint)
        for lint in selected
    ):
        raise ValueError("invalid production Clippy lint list")
    if scopes is None:
        if selected:
            raise ValueError("production Clippy lints require final test scope keys")
        return (all_targets,)
    denied = tuple(arg for lint in selected for arg in ("-D", lint))
    allowed = tuple(arg for lint in selected for arg in ("-A", lint))
    return (
        (*base, "--bin", "bus", "--locked", "--", "-D", "warnings", *denied),
        (*all_targets, *allowed),
    )


def windows_lint() -> int:
    """Use the same production/test policy for the cross-target check."""
    policy = tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))
    env = child_env(LIBGHOSTTY_VT_SIMD="false")
    return int(any([
        run(*argv, env=env)
        for argv in clippy_commands(policy, "x86_64-pc-windows-msvc")
    ]))


def head() -> str:
    return subprocess.check_output(
        ("git", "rev-parse", "HEAD"), cwd=ROOT, text=True
    ).strip()


def state() -> dict:
    value = json.loads((OUTPUT / "state.json").read_text(encoding="utf-8"))
    if value["head"] != head():
        raise ValueError(
            "coverage belongs to a different HEAD; run unit and integration again"
        )
    return value


def save_state(value: dict) -> None:
    (OUTPUT / "state.json").write_text(json.dumps(value), encoding="utf-8")


def profile_file_name(stage: str) -> str:
    # LLVM locks and online-merges four slots per binary instead of retaining
    # one multi-megabyte profile for every nextest/CLI process. Keep lanes
    # separate so CLI or integration cannot substitute for unit evidence.
    name = f"{ROOT.name}-{stage}-%4m.profraw"
    return str(Path(os.environ[PROFILE_DIR]) / name) if os.environ.get(PROFILE_DIR) else name


def rust_test(
    *targets: str, fresh: bool = False, in_process_server: bool | None = None
) -> int:
    stage = "unit" if fresh else "integration"
    env = child_env(LLVM_PROFILE_FILE_NAME=profile_file_name(stage))
    # --no-report already preserves profiles and rejects --no-clean. Reset just the raw
    # profiles once per round, keeping the instrumented build cache warm.
    if fresh and not os.environ.get(PROFILE_DIR) and run("cargo", "llvm-cov", "clean", "--profraw-only", env=env):
        return 1
    argv = [
        "cargo",
        "llvm-cov",
        "nextest" if shutil.which("cargo-nextest") else "test",
        "--locked",
        "--no-report",
        *COVERAGE_BUILD_FLAGS,
        *targets,
    ]
    if shutil.which("cargo-nextest"):
        argv += [
            "--no-fail-fast",
            "--status-level",
            "fail",
            "--final-status-level",
            "fail",
            "--failure-output",
            "final",
            "--success-output",
            "never",
        ]
        if in_process_server is not None:
            test_filter = f"test(/^{re.escape(IN_PROCESS_SERVER_TESTS)}/)"
            argv += ["-E", test_filter if in_process_server else f"not {test_filter}"]
    else:
        argv += ["--no-fail-fast", "--quiet"]
        if in_process_server is False:
            argv += ["--", "--skip", IN_PROCESS_SERVER_TESTS]
        elif in_process_server is True:
            # cargo-llvm-cov test accepts a libtest substring before the `--` delimiter.
            argv.append(IN_PROCESS_SERVER_TESTS)
    return run(*argv, env=env)


def file_lengths() -> int:
    policy = tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))
    scopes = configured_test_scopes(policy)
    sys.path.insert(0, str(ROOT))
    from tools.quality.scopes import is_test_path, production_line_count

    limit = policy["max_file_lines"]
    if not isinstance(limit, int) or limit <= 0:
        raise ValueError("invalid file-length limit")
    failures = []
    paths = python_files() + [
        path for folder in RUST_ROOTS for path in (ROOT / folder).rglob("*.rs")
    ]
    for path in sorted(paths):
        name = path.relative_to(ROOT).as_posix()
        if name in policy["generated_files"]:
            continue
        source = path.read_text(encoding="utf-8")
        lines = len(source.splitlines())
        if scopes is not None:
            if is_test_path(name, **scopes):
                continue
            if path.suffix == ".rs":
                lines = production_line_count(source, name, **scopes)
        if lines > limit:
            if name in policy.get("file_length_exemptions", {}):
                print(
                    f"file-length exemption: {name} ({lines} lines; split planned in restructure)"
                )
            else:
                failures.append(
                    f"{name}: {lines} lines exceeds {limit}; split the file"
                )
    for failure in failures:
        print(failure, file=sys.stderr)
    return int(bool(failures))


def lint() -> int:
    roots = python_roots()
    if not roots:
        raise ValueError("no Python source roots found")
    policy = tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))
    results = [
        run("cargo", "fmt", "--check"),
        *(run(*argv) for argv in clippy_commands(policy)),
        *(run(*argv) for argv in clippy_commands(policy, features=DEV_TOOLS_FEATURES)),
        run(
            sys.executable,
            "-m",
            "ruff",
            "check",
            "--isolated",
            "--select",
            "E9,F",
            *roots,
        ),
        run(sys.executable, "-m", "pytest", "-q", *ARCHITECTURE_TESTS),
        run(sys.executable, "-m", "tools.quality.placement", "--enforce"),
        file_lengths(),
        run(sys.executable, "-m", IMPORT_BOUNDARIES, "--enforce"),
    ]
    return int(any(results))


def llvm_cov_target(env) -> Path:
    """Mirror cargo-llvm-cov's output directory so tests run the binary it built."""
    if env.get("CARGO_LLVM_COV_TARGET_DIR"):
        return ROOT / env["CARGO_LLVM_COV_TARGET_DIR"]
    return ROOT / env.get("CARGO_TARGET_DIR", "target") / "llvm-cov-target"


def profile_directory(target: Path) -> Path:
    return Path(os.environ[PROFILE_DIR]) if os.environ.get(PROFILE_DIR) else target


def unit_profraw_files(target: Path) -> list[Path]:
    return sorted(
        path for path in profile_directory(target).glob("*.profraw")
        if path.name.startswith(f"{ROOT.name}-unit-")
        and path.is_file() and path.stat().st_size > 0
    )


def require_unit_profraw(value: dict) -> None:
    target = llvm_cov_target(os.environ).resolve()
    names = value.get("unit_profraw", [])
    if not isinstance(names, list) or not names:
        raise ValueError(
            "unit profraw is missing: the Rust unit run produced no coverage profiles; "
            "an uninstrumented cached binary may have been reused. Rerun the complete gate"
        )
    if value.get("profile_target") != str(target):
        raise ValueError("unit profraw target changed; rerun unit and integration")
    directory = profile_directory(target)
    if value.get("profile_dir", str(target)) != str(directory):
        raise ValueError("unit profraw run changed; rerun unit and integration")
    for name in names:
        if (
            not isinstance(name, str) or Path(name).name != name
            or not name.startswith(f"{ROOT.name}-unit-") or not name.endswith(".profraw")
        ):
            raise ValueError("invalid unit profraw evidence; rerun the complete gate")
        path = directory / name
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"unit profraw is missing or empty: {path}; rerun the complete gate")


def unit() -> int:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    # Never let a failed/new round reuse yesterday's successful report.
    for name in ("rust.lcov", "summary.json"):
        (OUTPUT / name).unlink(missing_ok=True)
    target = llvm_cov_target(os.environ)
    value = {
        "head": head(), "unit": False, "integration": False,
        "profile_target": str(target.resolve()), "profile_dir": str(profile_directory(target)),
        "unit_profraw": [],
    }
    save_state(value)
    rust = rust_test("--bin", "bus", fresh=True, in_process_server=False)
    # Checkpoint before CLI/Python/integration can create their own profiles.
    value["unit_profraw"] = [path.name for path in unit_profraw_files(target)]
    if not value["unit_profraw"]:
        print(
            "Rust unit run produced no unit profraw; coverage cannot accept an "
            "uninstrumented cached test binary", file=sys.stderr,
        )
    cli = run(
        "cargo",
        "llvm-cov",
        "run",
        "--locked",
        "--no-report",
        *COVERAGE_BUILD_FLAGS,
        "--bin",
        "bus",
        "--",
        "--help",
        env=child_env(LLVM_PROFILE_FILE_NAME=profile_file_name("cli")),
    )
    test_env = child_env(
        BUS_TEST_BINARY=str(
            target / "debug" / ("bus.exe" if os.name == "nt" else "bus")
        ),
        LLVM_PROFILE_FILE=str(target / profile_file_name("cli")),
    )
    tests = [str(path.relative_to(ROOT)) for path in python_files() if is_test(path)]
    if not tests:
        raise ValueError("no Python tests found")
    # Python here is developer and agent tooling, not the product: its tests
    # must pass, but a percentage target would only reward coverage-only tests.
    python = run(sys.executable, "-m", "pytest", "-q", *tests, env=test_env)
    value["unit"] = not (rust or cli or python or not value["unit_profraw"])
    save_state(value)
    return int(not value["unit"])


def dev_tools_test() -> int:
    """Run the dev variant's test tooling: the CLI and driver targets, then the
    driver's unit tests. Coverage measures only the release variant."""
    targets = [arg for target in DEV_TOOLS_TEST_TARGETS for arg in ("--test", target)]
    if shutil.which("cargo-nextest"):
        common = (
            "--no-fail-fast", "--status-level", "fail", "--final-status-level", "fail",
            "--failure-output", "final", "--success-output", "never",
        )
        results = [
            run("cargo", "nextest", "run", "--locked", *DEV_TOOLS_FEATURES, *targets, *common),
            run(
                "cargo", "nextest", "run", "--locked", *DEV_TOOLS_FEATURES, "--bin", "bus",
                *common, "-E", f"test(/^{re.escape(DEV_TOOLS_UNIT_PREFIX)}/)",
            ),
        ]
    else:
        results = [
            run("cargo", "test", "--locked", *DEV_TOOLS_FEATURES, *targets, "--no-fail-fast", "--quiet"),
            run(
                "cargo", "test", "--locked", *DEV_TOOLS_FEATURES, "--bin", "bus",
                "--no-fail-fast", "--quiet", DEV_TOOLS_UNIT_PREFIX,
            ),
        ]
    return int(any(results))


def integration() -> int:
    value = state()
    results = [
        rust_test("--test", "*"),
        rust_test("--bin", "bus", in_process_server=True),
        dev_tools_test(),
    ]
    value["integration"] = not any(results)
    save_state(value)
    return int(not value["integration"])


def rust_lines(report: Path) -> tuple[int, int]:
    # LLVM's DA records already define executable lines. Only remove cfg(test) modules;
    # their high coverage must not inflate the production baseline.
    sys.path.insert(0, str(ROOT))
    from tools.quality.rust_source import mask_comments_and_literals
    from tools.quality.scopes import is_test_path, rust_test_line_numbers

    scopes = configured_test_scopes(tomllib.loads(LINT_POLICY.read_text(encoding="utf-8")))

    covered = total = 0
    excluded: set[int] = set()
    in_source = False
    for line in report.read_text(encoding="utf-8").splitlines():
        if line.startswith("SF:"):
            path = Path(line[3:])
            if not path.is_absolute():
                path = ROOT / path
            in_source = path.is_relative_to(ROOT / RUST_SOURCE_ROOT) and not re.search(
                RUST_EXCLUDE, path.as_posix()
            )
            excluded = set()
            if scopes is not None and path.is_relative_to(ROOT / RUST_SOURCE_ROOT):
                relative = path.relative_to(ROOT)
                in_source = (
                    in_source and relative.as_posix() not in GENERATED_RUST
                    and "vendor" not in relative.parts and path.name != "build.rs"
                    and not is_test_path(relative, **scopes)
                )
            if in_source:
                source = path.read_text(encoding="utf-8")
                if scopes is not None:
                    excluded.update(rust_test_line_numbers(source))
                    continue
                code = mask_comments_and_literals(source)
                for match in LEGACY_TEST_MODULE.finditer(code):
                    depth, end = 0, match.end() - 1
                    while end < len(code):
                        depth += (code[end] == "{") - (code[end] == "}")
                        end += 1
                        if depth == 0:
                            break
                    excluded.update(
                        range(
                            code.count("\n", 0, match.start()) + 1,
                            code.count("\n", 0, end) + 2,
                        )
                    )
        elif in_source and line.startswith("DA:"):
            number, count, *_ = line[3:].split(",")
            if int(number) not in excluded:
                total += 1
                covered += int(int(count) > 0)
    return covered, total


def check_percent(label: str, covered: int, total: int, threshold: float) -> dict:
    if total <= 0 or not 0 < threshold <= 100:
        raise ValueError(f"{label}: missing executable coverage or invalid threshold")
    percent = covered * 100 / total
    print(
        f"{label} lines: {covered}/{total} = {percent:.2f}% (minimum {threshold:.2f}%)"
    )
    return {
        "covered": covered,
        "total": total,
        "percent": percent,
        "minimum": threshold,
        "pass": percent >= threshold,
    }


def coverage() -> int:
    value = state()
    require_unit_profraw(value)
    if not (value["unit"] and value["integration"]):
        raise ValueError(
            "coverage requires successful unit and integration checks from this round"
        )
    with merged_profile(ROOT, llvm_cov_target(os.environ), dict(os.environ), run):
        if run(
            "cargo",
            "llvm-cov",
            "report",
            "--lcov",
            "--output-path",
            str(OUTPUT / "rust.lcov"),
            "--ignore-filename-regex",
            RUST_EXCLUDE,
        ):
            return 1
    policy = json.loads(POLICY.read_text(encoding="utf-8"))
    # Only Rust, the product, has a floor; Python tests run in the unit lane
    # without one (see unit()).
    summary = {
        "head": value["head"],
        "rust": check_percent(
            "Rust", *rust_lines(OUTPUT / "rust.lcov"), policy["rust_line_percent"]
        ),
    }
    reference = float(policy["rust_line_percent"])
    reference_kind = "policy minimum (no successful local report yet)"
    previous = OUTPUT / "last-successful.json"
    if previous.exists():
        prior = json.loads(previous.read_text(encoding="utf-8"))["rust"]
        percent = prior["percent"]
        if type(percent) not in (int, float) or not 0 <= percent <= 100 or prior["pass"] is not True:
            raise ValueError(f"invalid successful coverage reference: {previous}")
        reference = percent
        reference_kind = "last successful local report"
    measured = summary["rust"]
    measured["reference_percent"] = reference
    measured["reference_kind"] = reference_kind
    drop = reference - measured["percent"]
    if drop > 20:
        measured["failure"] = (
            f"Rust coverage dropped by {drop:.2f} percentage points from {reference:.2f}% "
            f"({reference_kind}); more than 20 points suggests missing instrumentation "
            "or an incomplete test run. Inspect unit profraw and rerun the complete gate"
        )
        measured["pass"] = False
    rendered = json.dumps(summary, indent=2) + "\n"
    (OUTPUT / "summary.json").write_text(
        rendered, encoding="utf-8"
    )
    if "failure" in measured:
        raise ValueError(measured["failure"])
    if measured["pass"]:
        pending = OUTPUT / "last-successful.json.tmp"
        pending.write_text(rendered, encoding="utf-8")
        pending.replace(previous)
    return int(not summary["rust"]["pass"])


def run_lane(lane: str) -> int:
    required = ("ruff", "pytest") if lane == "lint" else ("pytest",)
    if any(importlib.util.find_spec(name) is None for name in required):
        raise ValueError("missing Python gate tools; install scripts/requirements.txt from this skill")
    if lane in ("unit", "integration", "coverage", "ci") and not shutil.which("cargo-llvm-cov"):
        raise ValueError("install cargo-llvm-cov and rustup component add llvm-tools-preview")
    checks = {
        "lint": lint, "unit": unit, "integration": integration,
        "coverage": coverage, "windows-lint": windows_lint,
    }
    return int(any([checks[name]() for name in LANES])) if lane == "ci" else checks[lane]()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane", choices=(*LANES, "ci", "windows-lint"))
    args = parser.parse_args(argv)
    try:
        with interrupt_signals():
            if args.lane not in ("unit", "integration", "coverage", "ci"):
                return run_lane(args.lane)
            with Run(ROOT, OUTPUT, args.lane) as profiles:
                result = run_lane(args.lane)
                profiles.finish(result)
                return result
    except Interrupted as error:
        print("Bus quality gate interrupted; run profiles removed", file=sys.stderr)
        return 128 + error.signum
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Bus quality gate failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
