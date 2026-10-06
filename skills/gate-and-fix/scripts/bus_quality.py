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

ROOT = Path(__file__).resolve().parents[3]
OUTPUT = ROOT / "temp/gate-and-fix/coverage"
CONFIG = ROOT / "skills/gate-and-fix/references/python-coverage.ini"
POLICY = ROOT / "skills/gate-and-fix/references/coverage-policy.json"
LINT_POLICY = ROOT / "skills/gate-and-fix/references/lint-policy.toml"
LANES = ("lint", "unit", "integration", "coverage")
PYTHON_ROOTS = ("scripts", "skills", "cli_extensions")
RUST_EXCLUDE = r"/(tests|vendor)/|/(tests|test_support)\.rs$|/build\.rs$|/src/ghostty/bindings\.rs$"


def run(*argv: str, env: dict | None = None) -> int:
    print("+ " + subprocess.list2cmdline(argv), flush=True)
    return subprocess.run(argv, cwd=ROOT, env=env).returncode


def python_files() -> list[Path]:
    return sorted(path for name in PYTHON_ROOTS for path in (ROOT / name).rglob("*.py"))


def is_test(path: Path) -> bool:
    return (
        path.name.startswith("test_")
        or path.name.endswith("_test.py")
        or "tests" in path.parts
    )


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


def rust_test(
    *targets: str, fresh: bool = False, test_filter: str | None = None
) -> int:
    # --no-report already preserves profiles and rejects --no-clean. Reset just the raw
    # profiles once per round, keeping the instrumented build cache warm.
    if fresh and run("cargo", "llvm-cov", "clean", "--profraw-only"):
        return 1
    argv = [
        "cargo",
        "llvm-cov",
        "nextest" if shutil.which("cargo-nextest") else "test",
        "--locked",
        "--no-report",
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
        if test_filter:
            argv += ["-E", test_filter]
    else:
        argv += ["--no-fail-fast", "--quiet"]
        if test_filter == "not test(/^server::headless::/)":
            argv += ["--", "--skip", "server::headless::"]
        elif test_filter:
            # cargo-llvm-cov test accepts a libtest substring before the `--` delimiter.
            argv.append("server::headless::")
    return run(*argv)


def file_lengths() -> int:
    policy = tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))
    limit = policy["max_file_lines"]
    if not isinstance(limit, int) or limit <= 0:
        raise ValueError("invalid file-length limit")
    failures = []
    paths = python_files() + [
        path for folder in ("src", "tests") for path in (ROOT / folder).rglob("*.rs")
    ]
    for path in sorted(paths):
        name = path.relative_to(ROOT).as_posix()
        if name in policy["generated_files"]:
            continue
        lines = len(path.read_text(encoding="utf-8").splitlines())
        if lines > limit:
            if name in policy["file_length_exemptions"]:
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
    results = [
        run("cargo", "fmt", "--check"),
        run("cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"),
        run(
            sys.executable,
            "-m",
            "ruff",
            "check",
            "--isolated",
            "--select",
            "E9,F",
            *PYTHON_ROOTS,
        ),
        run(sys.executable, "-m", "unittest", "scripts.test_ui_hot_path_architecture"),
        file_lengths(),
    ]
    return int(any(results))


def unit() -> int:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    # Never let a failed/new round reuse yesterday's successful report or Python profiles.
    for path in OUTPUT.glob(".coverage*"):
        path.unlink()
    for name in ("rust.lcov", "summary.json"):
        (OUTPUT / name).unlink(missing_ok=True)
    value = {"head": head(), "unit": False, "integration": False}
    save_state(value)
    rust = rust_test(
        "--bin", "bus", fresh=True, test_filter="not test(/^server::headless::/)"
    )
    cli = run(
        "cargo",
        "llvm-cov",
        "run",
        "--locked",
        "--no-report",
        "--bin",
        "bus",
        "--",
        "--help",
    )
    target = Path(
        os.environ.get("CARGO_LLVM_COV_TARGET_DIR", ROOT / "target/llvm-cov-target")
    )
    test_env = dict(
        os.environ,
        BUS_TEST_BINARY=str(
            target / "debug" / ("bus.exe" if os.name == "nt" else "bus")
        ),
        LLVM_PROFILE_FILE=str(target / f"{ROOT.name}-%p-%11m.profraw"),
    )
    tests = [str(path.relative_to(ROOT)) for path in python_files() if is_test(path)]
    if not tests:
        raise ValueError("no Python tests found")
    python = run(
        sys.executable,
        "-m",
        "coverage",
        "run",
        f"--rcfile={CONFIG}",
        "-m",
        "pytest",
        "-q",
        *tests,
        env=test_env,
    )
    value["unit"] = not (rust or cli or python)
    save_state(value)
    return int(not value["unit"])


def integration() -> int:
    value = state()
    results = [
        rust_test("--test", "*"),
        rust_test("--bin", "bus", test_filter="test(/^server::headless::/)"),
    ]
    value["integration"] = not any(results)
    save_state(value)
    return int(not value["integration"])


def rust_lines(report: Path) -> tuple[int, int]:
    # LLVM's DA records already define executable lines. Only remove cfg(test) modules;
    # their high coverage must not inflate the production baseline.
    sys.path.insert(0, str(ROOT))
    from scripts.test_ui_hot_path_architecture import (
        TEST_MODULE,
        mask_comments_and_literals,
    )

    covered = total = 0
    excluded: set[int] = set()
    in_source = False
    for line in report.read_text(encoding="utf-8").splitlines():
        if line.startswith("SF:"):
            path = Path(line[3:])
            if not path.is_absolute():
                path = ROOT / path
            in_source = path.is_relative_to(ROOT / "src") and not re.search(
                RUST_EXCLUDE, str(path)
            )
            excluded = set()
            if in_source:
                code = mask_comments_and_literals(path.read_text(encoding="utf-8"))
                for match in TEST_MODULE.finditer(code):
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


def python_lines(exclusions: dict) -> tuple[int, int]:
    from coverage import Coverage

    cov = Coverage(config_file=str(CONFIG))
    cov.load()
    covered = total = 0
    for path in python_files():
        if is_test(path) or path.relative_to(ROOT).as_posix() in exclusions:
            continue
        # analysis2 includes executable statements even for files never imported by tests.
        _, statements, _, missing, _ = cov.analysis2(str(path))
        total += len(statements)
        covered += len(statements) - len(missing)
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
    if not (value["unit"] and value["integration"]):
        raise ValueError(
            "coverage requires successful unit and integration checks from this round"
        )
    results = [
        run(
            "cargo",
            "llvm-cov",
            "report",
            "--lcov",
            "--output-path",
            str(OUTPUT / "rust.lcov"),
            "--ignore-filename-regex",
            RUST_EXCLUDE,
        ),
        run(sys.executable, "-m", "coverage", "combine", f"--rcfile={CONFIG}"),
    ]
    if any(results):
        return 1
    policy = json.loads(POLICY.read_text(encoding="utf-8"))
    summary = {
        "head": value["head"],
        "rust": check_percent(
            "Rust", *rust_lines(OUTPUT / "rust.lcov"), policy["rust_line_percent"]
        ),
        "python": check_percent(
            "Python",
            *python_lines(policy["python_exclude"]),
            policy["python_line_percent"],
        ),
    }
    (OUTPUT / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8"
    )
    return int(not all(summary[name]["pass"] for name in ("rust", "python")))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane", choices=(*LANES, "ci"))
    args = parser.parse_args(argv)
    try:
        required = ("ruff",) if args.lane == "lint" else ("coverage", "pytest")
        if any(importlib.util.find_spec(name) is None for name in required):
            raise ValueError(
                "missing Python gate tools; install scripts/requirements.txt from this skill"
            )
        if args.lane != "lint" and not shutil.which("cargo-llvm-cov"):
            raise ValueError(
                "install cargo-llvm-cov and rustup component add llvm-tools-preview"
            )
        checks = {
            "lint": lint,
            "unit": unit,
            "integration": integration,
            "coverage": coverage,
        }
        return (
            int(any([checks[name]() for name in LANES]))
            if args.lane == "ci"
            else checks[args.lane]()
        )
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Bus quality gate failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
