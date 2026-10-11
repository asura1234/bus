---
name: e2e-test-tui
description: Use to test the Bus TUI end to end like a manual QA tester, against a QA test doc (default tests/e2e-tui/app.md, optionally only some case IDs). Builds the dev-tools variant, drives a fresh scratch Bus through `bus --dev tui` (never the running Bus), judges every step against the doc from screen text, styled cells, screenshots and recorded effects, and writes a run report with PASS/FAIL/SKIP per step and a GREEN/RED verdict.
---

# E2E Test TUI

Read [guide.md](guide.md), [e2e-run-report-format.md](references/e2e-run-report-format.md) and
`docs/tui-driver.md` completely before driving anything. Read the whole test doc, including its
run contract, setup recipes, fixtures and decisions.

```text
INPUT = [<test doc path>] [--cases ID,ID,...]

repo   = git rev-parse --show-toplevel
doc    = INPUT test doc path, otherwise tests/e2e-tui/app.md
IF doc is not a file: STOP "No test doc at <doc>."
IF BUS_DATA_DIR, BUS_SESSION_ID or any HERDR_* is set in your environment
  Unset them for every command below. The driver refuses to touch a live Bus anyway.

-- Build the dev variant once; ./run tui builds into its own tui-driver dir, never target/debug.
Run: ./run tui list
IF it fails: STOP with its output; the build is broken, nothing was tested.
T = the driver command: `./run tui` (each call rebuilds only if sources changed)

-- Start the report from the doc; never write case or step headings by hand.
Run:
  python3 skills/e2e-test-tui/scripts/e2e_report.py init --doc <doc> [--cases <IDs>] \
    --commit "$(git rev-parse HEAD)" --platform "<OS>, Ghostty <version> for --real, <size>"
report = its stdout line; run_dir = dirname(report)

FOR EACH case in the report, in order:
  name = case ID lowercased (e.g. app-07)
  Run: T start --name <name> --size <doc size, default 120x40> --run-dir <run_dir>/sessions
  IF start fails: mark every step of the case FAIL with the start error; continue.
  Record the start output's run_dir as the case's `Session`.
  Do the case's preconditions/setup with the doc's recipes, through the UI the doc names.
    Use `T bus --name <name> ...` only where the doc allows fixtures or read-only state.
  FOR EACH numbered step:
    Do the step's actions exactly as written (keys, clicks, typing), one driver call each.
    Judge the expected result with the evidence the doc asks for (guide.md "Judging"):
      text/state → snapshot, find, wait --text, wait --state, `bus state`/`history`
      colours/attributes → cells, wait --style, wait --changed
      visual claims → screenshot (cells) or screenshot --real (images); open the PNG
        and write what you saw
      opens/sounds/notifications → effects, wait --effect
    Write the step's Result and Evidence into the report immediately.
    IF the result does not match: FAIL with every FAIL field (format doc), a screenshot,
      the session trace path and the effects; guess the fault per guide.md "Fault guess".
    IF the doc allows SKIP for evidence you cannot capture: SKIP with that reason.
    Never retry a failed step until it passes; a flaky pass is a FAIL with both runs noted.
    IF a later step depends on a failed one: still attempt it; record what happened.
  Run: T stop --name <name>   (always, also after failures)

Run: python3 skills/e2e-test-tui/scripts/e2e_report.py check --doc <doc> <report>
IF exit code 1: fix the report's form (never its results) and rerun check.
Append check's output under `## Summary` at the end of the report.

RETURN, verbatim: "<Verdict line> — <report path>" followed by check's per-case lines and,
for each FAIL, one line: <case> step <n>: <seen> (fault guess: <owner>).
```

Driver or skill bugs found mid-run (the driver misreads the screen, a verb fails though the UI
is fine) are `skill-driver` FAILs in the report; fixing them is separate work, after the run.
