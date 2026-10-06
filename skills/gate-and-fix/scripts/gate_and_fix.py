#!/usr/bin/env python3
"""Run the applicable gates in parallel and produce complete evidence for the remediation loop."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import time
import uuid
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from gate_artifact import GateResult, _parse_round, render_round, validate_round


CI_TOOLS = frozenset({"just", "cargo-nextest"})
QUALITY_SCRIPT = "skills/gate-and-fix/scripts/bus_quality.py"
QUALITY_RECIPES = {
    "lint": "lint",
    "unit": "unit-test",
    "integration": "integration-test",
    "coverage": "coverage",
}
MAX_PARALLEL_GATES = 4


@dataclass(frozen=True)
class Gate:
    name: str
    argv: tuple[str, ...]
    resources: frozenset[str] = frozenset()
    requires_exclusive_execution: bool = False


def _validate_paths(changed_files: Sequence[str]) -> tuple[str, ...]:
    changed = tuple(sorted(set(changed_files)))
    if not changed:
        raise ValueError("no changes against the base; cannot build gates")
    if any(
        not path
        or Path(path).is_absolute()
        or any(part in {"", ".", ".."} for part in path.split("/"))
        for path in changed
    ):
        raise ValueError("changed files must be normalized repository-relative paths")
    return changed


def select_gates(
    changed_files: Sequence[str],
    *,
    base: str,
    available_tools: frozenset[str] | None = None,
    python: str = sys.executable,
) -> list[Gate]:
    """Every committed diff gets all four checks; wrappers never change their scope."""
    _validate_paths(changed_files)
    tools = available_tools
    if tools is None:
        tools = frozenset(name for name in CI_TOOLS if shutil.which(name))
    gates = []
    for lane, recipe in QUALITY_RECIPES.items():
        argv = (
            ("just", "--set", "python", python, recipe)
            if CI_TOOLS.issubset(tools)
            else (python, QUALITY_SCRIPT, lane)
        )
        # Unit creates fresh profiles; integration extends them; coverage consumes them.
        gates.append(Gate(lane, argv, requires_exclusive_execution=True))
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
    """Run independent gates in parallel, serialize writable resources, and isolate exclusive gates."""
    if not gates:
        raise ValueError("at least one gate is required")
    if len({gate.name for gate in gates}) != len(gates):
        raise ValueError("duplicate gate name")

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
                raise ValueError("gate resource scheduling cannot make progress")
            completed, _ = wait(active, return_when=FIRST_COMPLETED)
            for future in completed:
                gate = active.pop(future)
                active_resources.difference_update(gate.resources)
                results[gate.name] = future.result()
    return [results[gate.name] for gate in gates]


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
    parser = argparse.ArgumentParser(description="Validate a gate-and-fix Markdown round artifact")
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
    parser = argparse.ArgumentParser(description="Print one complete log stream from a validated gate artifact")
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
            raise ValueError(f"artifact does not contain gate: {args.gate}")
    except (OSError, ValueError) as error:
        print(f"gate-and-fix show failed: {error}", file=sys.stderr)
        return 2
    sys.stdout.write(gate.stdout if args.stream == "stdout" else gate.stderr)
    return 0


def _list_main(argv: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(description="List the failed gate names of a validated gate artifact")
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
        description="Run the applicable gates in parallel and write the sole Markdown round artifact"
    )
    parser.add_argument("--repo", default=".")
    parser.add_argument("--base", required=True, help="immutable baseline commit after rebase")
    parser.add_argument("--round", required=True, type=int)
    parser.add_argument("--artifact-root", required=True, help="artifact root dedicated to this invocation")
    args = parser.parse_args(arguments)

    try:
        if args.round < 1:
            raise ValueError("round must be greater than 0")
        repo = Path(args.repo).resolve()
        if _git_output(repo, "status", "--porcelain", "--untracked-files=all"):
            raise ValueError(
                "worktree is not clean: commit or remove every tracked and untracked change before running gate-and-fix"
            )
        base = _git_output(repo, "rev-parse", "--verify", f"{args.base}^{{commit}}")
        head = _git_output(repo, "rev-parse", "HEAD")
        changed_files = _git_output(repo, "diff", "--name-only", f"{base}...HEAD").splitlines()
        gates = select_gates(
            changed_files,
            base=base,
            python=str(repo / "temp/gate-tools/python/bin/python")
            if (repo / "temp/gate-tools/python/bin/python").is_file() else sys.executable,
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
        outcome = validate_round(artifact_path.read_text(encoding="utf-8"), expected_base=base)
    except (OSError, ValueError) as error:
        print(f"gate-and-fix runner failed: {error}", file=sys.stderr)
        return 2

    print(artifact_path)
    return 0 if outcome == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
