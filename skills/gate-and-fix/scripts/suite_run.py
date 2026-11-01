"""Run conservative affected tests or the complete parallel Bus quality gate."""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import uuid
from contextlib import nullcontext
from dataclasses import asdict, replace
from pathlib import Path

import bus_quality as quality
from coverage_run import EXTERNAL_RUN, PROFILE_DIR, Run
from gate_and_fix import Gate, GateResult, run_gates
from gate_process import Interrupted, interrupt_signals
from suite_selection import Selection, changed_paths, select

COMMON = ("--no-fail-fast", "--status-level", "fail", "--final-status-level", "fail",
          "--failure-output", "final", "--success-output", "never")
SCRIPT = str(Path(__file__).with_name("bus_quality.py"))


def needs_profiles(selection: Selection) -> bool:
    # The feature build overwrites normal-target binaries. Keep default and
    # dev-tools in different Cargo outputs whenever both variants are selected.
    return selection.full or (selection.dev_tools and (selection.rust_filter != "none()" or selection.binary))


def nextest(*targets: str, coverage: bool = False, expression: str | None = None) -> tuple[str, ...]:
    prefix = ("cargo", "llvm-cov", "nextest", "--no-report", "--no-rustc-wrapper") if coverage else ("cargo", "nextest", "run")
    return (*prefix, "--locked", *targets, *COMMON,
            "--test-threads", os.environ.get("NEXTEST_TEST_THREADS", "4"),
            "--no-tests=fail", *(("-E", expression) if expression else ()))


def commands(selection: Selection) -> tuple[list[Gate], list[Gate]]:
    build, tests = [], []
    instrumented = needs_profiles(selection)
    rust = selection.rust_filter != "none()"
    env = quality.child_env()
    if selection.full and os.environ.get(PROFILE_DIR):
        # The scheduler already owns the lint worker's process group. Its cargo
        # children must stay in that group, following the existing quality runner contract.
        env[EXTERNAL_RUN] = os.environ[PROFILE_DIR]
    target = quality.llvm_cov_target(env) if instrumented else quality.ROOT / env.get("CARGO_TARGET_DIR", "target")
    if selection.full:
        build.append(Gate("lint", (sys.executable, SCRIPT, "lint"), env=env))
    if rust:
        # Explicit empty preflight compiles every default artifact once, including the
        # CLI used by Python, before concurrent nextest invocations read the cache.
        argv = nextest("--all-targets", coverage=instrumented, expression="none()")
        build.append(Gate("instrumented-build" if instrumented else "test-build",
                          tuple("--no-tests=pass" if arg == "--no-tests=fail" else arg for arg in argv),
                          env=quality.child_env(LLVM_PROFILE_FILE_NAME=quality.profile_file_name("build")) if instrumented else env))
        if selection.full:
            tests.extend([
                Gate("unit", nextest("--bin", "bus", coverage=True,
                     expression="not test(/^server::tests::/)"),
                     env=quality.child_env(LLVM_PROFILE_FILE_NAME=quality.profile_file_name("unit"))),
                Gate("integration", nextest("--all-targets", coverage=True,
                     expression="kind(test) | test(/^server::tests::/)"),
                     env=quality.child_env(LLVM_PROFILE_FILE_NAME=quality.profile_file_name("integration"))),
            ])
        else:
            tests.append(Gate("affected-rust", nextest("--all-targets", coverage=instrumented,
                              expression=selection.rust_filter), env=quality.child_env(
                                  LLVM_PROFILE_FILE_NAME=quality.profile_file_name("affected")) if instrumented else env))
    elif selection.binary:
        argv = (("cargo", "llvm-cov", "run", "--locked", "--no-report", "--no-rustc-wrapper", "--bin", "bus", "--", "--help")
                if instrumented else ("cargo", "build", "--locked", "--bin", "bus"))
        build.append(Gate("cli-build", argv, env=quality.child_env(
            LLVM_PROFILE_FILE_NAME=quality.profile_file_name("cli")) if instrumented else env))
    if selection.python:
        files = sorted({path.relative_to(quality.ROOT).as_posix()
                        for name in selection.python for path in (quality.ROOT / name).rglob('*_test.py')
                        if path.is_file()})
        if files:
            python_env = quality.child_env()
            if rust or selection.binary:
                python_env.update(BUS_TEST_BINARY=str(target / "debug" / ("bus.exe" if os.name == "nt" else "bus")))
            if instrumented:
                python_env["LLVM_PROFILE_FILE"] = str(target / quality.profile_file_name("cli"))
            tests.append(Gate("python", quality.python_test_command(files), env=python_env))
    if selection.dev_tools:
        tests.append(Gate("dev-tools", nextest("--features", "dev-tools", "--bin", "bus",
                          "--test", "cli", "--test", "tui-driver",
                          expression="binary(cli) | binary(tui-driver) | test(/^devtools::/)"), env=env))
    if selection.full:
        tests.insert(0, Gate("cli-smoke", (str(target / "debug" / ("bus.exe" if os.name == "nt" else "bus")), "--help"),
                          env=quality.child_env(LLVM_PROFILE_FILE=str(target / quality.profile_file_name("cli")))))
    normal = quality.ROOT / env.get("CARGO_TARGET_DIR", "target")
    if instrumented and target.resolve() == normal.resolve():
        # A user may explicitly co-locate outputs. Both Cargo writes and all
        # binary consumers then need serialization across the feature variants.
        shared = frozenset({"cargo-output"})
        build = [replace(gate, resources=shared) for gate in build]
        tests = [replace(gate, resources=shared) for gate in tests]
    return build, tests


def batch(gates: list[Gate], *, nested: bool = False) -> list[GateResult]:
    if not gates:
        return []
    print("Running concurrently: " + ", ".join(gate.name for gate in gates), flush=True)
    results = run_gates(gates, cwd=quality.ROOT, private_groups=not nested)
    results = [replace(result, exit_code=0, stdout=result.stdout +
                       "\npytest exit 5: empty selection, nothing to run\n")
               if result.name == "python" and result.exit_code == 5 else result for result in results]
    for result in results:
        print(f"{result.name}: {'PASS' if result.passed else 'FAIL'} ({result.duration_ms / 1000:.3f}s)", flush=True)
        if not result.passed:
            print(result.stdout, end="", flush=True)
            print(result.stderr, end="", file=sys.stderr, flush=True)
    return results


def execute(selection: Selection, *, nested: bool = False) -> int:
    started = time.monotonic()
    if selection.coverage_floor:
        for name in ("state.json", "rust.lcov", "summary.json"):
            (quality.OUTPUT / name).unlink(missing_ok=True)
    build, tests = commands(selection)
    results = batch(build, nested=nested)
    failed_build = any(not result.passed and result.name != "lint" for result in results)
    # A failed instrumented build must not let Python use yesterday's binary.
    if failed_build:
        tests = [gate for gate in tests if gate.name == "dev-tools"]
    results.extend(batch(tests, nested=nested))
    result = int(any(not entry.passed for entry in results))
    coverage_ms = None
    if selection.coverage_floor and not result:
        target = quality.llvm_cov_target(os.environ)
        quality.OUTPUT.mkdir(parents=True, exist_ok=True)
        quality.save_state({
            "head": quality.head(), "unit": True, "integration": True,
            "profile_target": str(target.resolve()),
            "profile_dir": str(quality.profile_directory(target)),
            "unit_profraw": [path.name for path in quality.unit_profraw_files(target)],
        })
        before = time.monotonic()
        try:
            result = quality.coverage()
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            print(f"Coverage floor: FAIL ({error})", file=sys.stderr, flush=True)
            result = 1
        coverage_ms = round((time.monotonic() - before) * 1000)
    elif selection.coverage_floor:
        print("Coverage floor: FAIL (build, lint or test prerequisites failed)", flush=True)
    else:
        print("Affected mode: coverage floor skipped; full gate required before merge", flush=True)
    report = {
        "head": quality.head(), "selection": asdict(selection), "exit_code": result,
        "duration_ms": round((time.monotonic() - started) * 1000),
        "coverage_ms": coverage_ms, "results": [asdict(entry) for entry in results],
    }
    output = quality.ROOT / "temp/run-test"
    output.mkdir(parents=True, exist_ok=True)
    path = output / f"run-{uuid.uuid4().hex}.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Test report: {path}\nElapsed: {report['duration_ms'] / 1000:.3f}s", flush=True)
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, epilog=
        "Python uses four file workers; set PYTEST_WORKERS=0 for serial diagnosis, "
        "or a positive integer to set the worker count.")
    parser.add_argument("--full", action="store_true", help="all tests, lint and fresh coverage floor")
    parser.add_argument("--base", default="origin/master", help="affected merge-base (default: origin/master)")
    parser.add_argument("--plan", action="store_true", help="print selection without building or running tests")
    args = parser.parse_args(argv)
    try:
        paths = [] if args.full else changed_paths(quality.ROOT, args.base)
        selection = select(paths, full=args.full, root=quality.ROOT)
        if args.plan:
            print(json.dumps({**asdict(selection), "head": quality.head(), "paths": paths,
                              "coverage_floor": args.full}, indent=2))
            return 0
        if not shutil.which("cargo-nextest") and (selection.full or selection.rust_filter != "none()" or selection.dev_tools):
            raise ValueError("install cargo-nextest to run Rust tests")
        if needs_profiles(selection) and not shutil.which("cargo-llvm-cov"):
            raise ValueError("install cargo-llvm-cov and llvm-tools-preview for full mode")
        # Conservative fallback collects profiles in the same isolated way, but
        # only --full can publish a coverage floor verdict.
        coverage = Run(quality.ROOT, quality.OUTPUT, "ci") if needs_profiles(selection) else nullcontext()
        with interrupt_signals(), coverage as profiles:
            return execute(selection, nested=bool(profiles and profiles.external))
    except Interrupted as error:
        print("Test run interrupted; children stopped and run profiles removed", file=sys.stderr)
        return 128 + error.signum
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Test run failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
