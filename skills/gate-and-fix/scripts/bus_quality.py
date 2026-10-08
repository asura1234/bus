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
POLICY = ROOT / "skills/gate-and-fix/references/coverage-policy.json"
LINT_POLICY = ROOT / "skills/gate-and-fix/references/lint-policy.toml"
LANES = ("lint", "unit", "integration", "coverage")
PYTHON_ROOTS = ("scripts", "skills", "cli_extensions", "tools", "packaging")
RUST_ROOTS = ("src", "tests")
RUST_SOURCE_ROOT = "src"
GENERATED_RUST = tuple(
    tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))["generated_files"]
)
RUST_EXCLUDE = "|".join(
    (
        r"/(tests|vendor)/",
        r"/(tests|test_support)\.rs$",
        r"/build\.rs$",
        *("/" + re.escape(path) + "$" for path in GENERATED_RUST),
    )
)
IN_PROCESS_SERVER_TESTS = "server::tests::"
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


def run(*argv: str, env: dict | None = None) -> int:
    print("+ " + subprocess.list2cmdline(argv), flush=True)
    return subprocess.run(argv, cwd=ROOT, env=env).returncode


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


def clippy_commands(policy: dict) -> tuple[tuple[str, ...], ...]:
    """Check production before allowing selected lints while compiling tests."""
    base = ("cargo", "clippy")
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
    *targets: str, fresh: bool = False, in_process_server: bool | None = None
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
    return run(*argv)


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
    results = [
        run("cargo", "fmt", "--check"),
        *(run(*argv) for argv in clippy_commands(
            tomllib.loads(LINT_POLICY.read_text(encoding="utf-8"))
        )),
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
        run(sys.executable, "-m", IMPORT_BOUNDARIES),
        run(sys.executable, "-m", "tools.quality.placement", "--enforce"),
        file_lengths(),
    ]
    return int(any(results))


def unit() -> int:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    # Never let a failed/new round reuse yesterday's successful report.
    for name in ("rust.lcov", "summary.json"):
        (OUTPUT / name).unlink(missing_ok=True)
    value = {"head": head(), "unit": False, "integration": False}
    save_state(value)
    rust = rust_test("--bin", "bus", fresh=True, in_process_server=False)
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
    # Python here is developer and agent tooling, not the product: its tests
    # must pass, but a percentage target would only reward coverage-only tests.
    python = run(sys.executable, "-m", "pytest", "-q", *tests, env=test_env)
    value["unit"] = not (rust or cli or python)
    save_state(value)
    return int(not value["unit"])


def integration() -> int:
    value = state()
    results = [
        rust_test("--test", "*"),
        rust_test("--bin", "bus", in_process_server=True),
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
                    relative.as_posix() not in GENERATED_RUST
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
    if not (value["unit"] and value["integration"]):
        raise ValueError(
            "coverage requires successful unit and integration checks from this round"
        )
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
    (OUTPUT / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8"
    )
    return int(not summary["rust"]["pass"])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane", choices=(*LANES, "ci"))
    args = parser.parse_args(argv)
    try:
        required = ("ruff", "pytest") if args.lane == "lint" else ("pytest",)
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
