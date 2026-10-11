#!/usr/bin/env python3
"""Run-report skeleton and checker for /e2e-test-tui.

`init` writes `report.md` with one PENDING step per numbered step of the chosen
cases of a test doc. `check` validates a filled report against the same doc
(references/e2e-run-report-format.md) and prints per-case results and the
verdict. Exit codes: 0 valid report, 1 malformed or incomplete, 2 usage.
"""
from __future__ import annotations

import argparse
import re
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

RESULTS = ("PASS", "FAIL", "SKIP", "PENDING")
FAULTS = ("product", "test-doc", "skill-driver")
FAIL_FIELDS = ("Expected", "Seen", "Trace", "Screenshot", "Effects", "Fault guess", "Fault evidence")
CASE_HEADING = re.compile(r"^### (?P<id>[A-Z][A-Z0-9]*-\d+) — (?P<title>.+)$")
STEP_LINE = re.compile(r"^(?P<n>\d+)\. ")


@dataclass
class Case:
    id: str
    title: str
    steps: list[int] = field(default_factory=list)


def parse_doc(text: str) -> list[Case]:
    """Cases are `### ID — title` headings; their steps are top-level `N. ` lines."""
    cases: list[Case] = []
    for line in text.splitlines():
        heading = CASE_HEADING.match(line)
        if heading:
            cases.append(Case(heading["id"], heading["title"].strip()))
        elif line.startswith("#"):
            if cases and not line.startswith("### "):
                cases.append(Case("", ""))  # a section after the cases ends the last one
        elif cases and cases[-1].id and (step := STEP_LINE.match(line)):
            cases[-1].steps.append(int(step["n"]))
    found = [case for case in cases if case.id]
    for case in found:
        # A case with nothing to run would otherwise score PASS untouched.
        if not case.steps:
            raise ValueError(f"{case.id}: no numbered steps")
        if case.steps != list(range(1, len(case.steps) + 1)):
            raise ValueError(f"{case.id}: steps are not numbered 1..n: {case.steps}")
    ids = [case.id for case in found]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate case IDs in the doc")
    return found


def select(cases: list[Case], wanted: str | None) -> list[Case]:
    if not wanted:
        return cases
    ids = [item.strip() for item in wanted.split(",") if item.strip()]
    known = {case.id: case for case in cases}
    missing = [item for item in ids if item not in known]
    if missing:
        raise ValueError(f"unknown case IDs: {', '.join(missing)}")
    return [case for case in cases if case.id in ids]


def render_skeleton(doc: str, cases: list[Case], commit: str, started: str, platform: str) -> str:
    lines = [
        "# E2E TUI Run",
        "",
        f"- Doc: `{doc}`",
        f"- Commit: `{commit}`",
        f"- Started: `{started}`",
        f"- Platform: {platform}",
        f"- Cases: `{', '.join(case.id for case in cases)}`",
    ]
    for case in cases:
        lines += ["", f"## {case.id} — {case.title}", "", "- Session: `none`"]
        for step in case.steps:
            lines += ["", f"### Step {step}", "", "- Result: `PENDING`", "- Evidence: none yet"]
    return "\n".join(lines) + "\n"


def fields_of(block: list[str]) -> list[tuple[str, str]]:
    found = []
    for line in block:
        match = re.match(r"^- ([A-Za-z ]+): (.*)$", line)
        if match:
            found.append((match[1], match[2].strip()))
    return found


def unquote(value: str) -> str:
    return value[1:-1] if len(value) > 1 and value[0] == value[-1] == "`" else value


def check(report: str, cases: list[Case]) -> tuple[list[str], list[str]]:
    """Returns (errors, summary lines)."""
    errors: list[str] = []
    header = re.search(r"^- Cases: `(?P<ids>[^`]*)`$", report, re.M)
    if not header:
        return ["missing `- Cases:` line"], []
    listed = [item.strip() for item in header["ids"].split(",") if item.strip()]
    try:
        chosen = select(cases, ",".join(listed))
    except ValueError as error:
        return [str(error)], []
    if [case.id for case in chosen] != listed:
        errors.append("Cases must be in doc order")
    sections = re.split(r"^## ", report, flags=re.M)[1:]
    if sections and sections[-1].startswith("Summary\n"):
        sections.pop()  # the runner appends check's own output; checking it again must not fail
    if [section.split(" — ", 1)[0] for section in sections] != listed:
        return errors + ["case sections do not match `Cases` one to one, in order"], []
    summary = []
    for case, section in zip(chosen, sections):
        steps = re.split(r"^### Step ", section, flags=re.M)[1:]
        numbers = [step.split("\n", 1)[0].strip() for step in steps]
        if numbers != [str(n) for n in case.steps]:
            errors.append(f"{case.id}: steps {numbers} differ from the doc's {case.steps}")
            continue
        session = dict(fields_of(section.split("\n### ", 1)[0].splitlines())).get("Session", "")
        if unquote(session) in ("", "none"):
            errors.append(f"{case.id}: Session names no driver run folder")
        results = []
        for number, step in zip(numbers, steps):
            where = f"{case.id} step {number}"
            fields = fields_of(step.splitlines())
            names = [name for name, _ in fields]
            repeated = sorted({name for name in names if names.count(name) > 1})
            if repeated:
                # A second Result or Evidence must not hide the first.
                errors.append(f"{where}: repeats {', '.join(repeated)}")
            values = dict(fields)
            result = unquote(values.get("Result", ""))
            results.append(result)
            if result not in RESULTS:
                errors.append(f"{where}: Result {result!r} is not one of {RESULTS}")
                continue
            if result == "PENDING":
                errors.append(f"{where}: still PENDING")
            if not values.get("Evidence") or values["Evidence"] == "none yet":
                errors.append(f"{where}: Evidence is empty")
            extra_fail = [name for name in FAIL_FIELDS if name in values]
            if result == "FAIL":
                missing = [name for name in FAIL_FIELDS if not values.get(name)]
                if missing:
                    errors.append(f"{where}: FAIL lacks {', '.join(missing)}")
                elif unquote(values["Fault guess"]) not in FAULTS:
                    errors.append(f"{where}: Fault guess must be one of {FAULTS}")
            elif extra_fail:
                errors.append(f"{where}: only FAIL steps carry {', '.join(extra_fail)}")
            if result == "SKIP" and not values.get("Reason"):
                errors.append(f"{where}: SKIP needs a Reason")
            if result != "SKIP" and "Reason" in values:
                errors.append(f"{where}: only SKIP steps carry a Reason")
        summary.append(f"{case.id} {case_result(results)}")
    return errors, summary


def case_result(results: list[str]) -> str:
    if "FAIL" in results:
        return "FAIL"
    if all(result == "PASS" for result in results):
        return "PASS"
    if all(result == "SKIP" for result in results):
        return "SKIP"
    return "PARTIAL"


def verdict(summary: list[str]) -> str:
    """GREEN only when every step ran and passed; any FAIL is RED; else INCOMPLETE."""
    results = [line.rsplit(" ", 1)[1] for line in summary]
    if "FAIL" in results:
        return "RED"
    if all(result == "PASS" for result in results):
        return "GREEN"
    skipped = sum(result != "PASS" for result in results)
    return f"INCOMPLETE ({skipped} case{'s' if skipped != 1 else ''} not fully run)"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    init = commands.add_parser("init")
    init.add_argument("--doc", required=True)
    init.add_argument("--cases")
    init.add_argument("--commit", required=True)
    init.add_argument("--platform", required=True)
    init.add_argument("--runs-root", default="temp/e2e-tui/runs")
    checker = commands.add_parser("check")
    checker.add_argument("--doc", required=True)
    checker.add_argument("report")
    args = parser.parse_args(argv)
    try:
        cases = parse_doc(Path(args.doc).read_text(encoding="utf-8"))
        if not cases:
            raise ValueError(f"{args.doc} has no `### ID — title` cases")
        if args.command == "init":
            chosen = select(cases, args.cases)
            started = time.strftime("%Y%m%d-%H%M%S", time.gmtime())
            run_dir = Path(args.runs_root) / started
            run_dir.mkdir(parents=True, exist_ok=False)
            report = run_dir / "report.md"
            report.write_text(
                render_skeleton(args.doc, chosen, args.commit, started, args.platform),
                encoding="utf-8",
            )
            sys.stdout.write(f"{report}\n")
            return 0
        errors, summary = check(Path(args.report).read_text(encoding="utf-8"), cases)
    except (OSError, ValueError) as error:
        sys.stderr.write(f"e2e_report: {error}\n")
        return 2
    if errors:
        sys.stderr.write("".join(f"invalid: {error}\n" for error in errors))
        return 1
    sys.stdout.write("".join(f"{line}\n" for line in summary))
    sys.stdout.write(f"Verdict: {verdict(summary)}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
