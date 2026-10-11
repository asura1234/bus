# E2E TUI Run Report Format

`scripts/e2e_report.py` writes the skeleton of this UTF-8 Markdown artifact from the test doc
(`init`), and is its sole validator and verdict renderer (`check`). The runner fills in results
between those two calls; it never adds, removes or reorders cases or steps.

````markdown
# E2E TUI Run

- Doc: `<test doc path, as passed to init>`
- Commit: `<commit SHA the driver was built from>`
- Started: `<UTC timestamp, YYYYMMDD-HHMMSS>`
- Platform: <one line: OS, terminal used for --real, driver size>
- Cases: `<comma-separated case IDs, in doc order>`

## <CASE-ID> — <case title from the doc>

- Session: `<driver run folder of this case's scratch session>`

### Step <n>

- Result: `PASS|FAIL|SKIP|PENDING`
- Evidence: <what was captured and how it matched; paths of screenshots or effects>
````

A `FAIL` step adds, in this order:

```markdown
- Expected: <the doc's expected result for the step>
- Seen: <what the runner captured instead>
- Trace: `<path of the session's trace.ndjson>`
- Screenshot: `<path>` or `none`
- Effects: <recorded effects that bear on the step, or `none`>
- Fault guess: `product|test-doc|skill-driver`
- Fault evidence: <why the runner guesses that owner>
```

A `SKIP` step adds `- Reason: <why the evidence could not be captured, quoting the doc's SKIP allowance>`.

Rules `check` enforces:

- The doc has at least one numbered step per case; `init` and `check` refuse a case with none.
- Every case listed in `Cases` has exactly one `##` section, in that order, and every step the
  doc numbers for it has exactly one `### Step <n>` section, in order.
- Every case names its driver run folder in `Session` (not `none`).
- No field appears twice within a step.
- `Result` is one of the four values; `PENDING` (written by `init`) fails `check`.
- `Evidence` is non-empty for every step. `FAIL` steps carry every FAIL field; `SKIP` steps carry
  `Reason`; `PASS` steps carry neither.
- `Fault guess` is advisory input for triage, never a verdict.

`check` prints one line per case (`<CASE-ID> PASS|FAIL|SKIP|PARTIAL`: FAIL if any step failed,
PASS if every step passed, SKIP if every step was skipped, else PARTIAL) and a final line:
`Verdict: GREEN` when every case is PASS, `Verdict: RED` when any case FAILs, otherwise
`Verdict: INCOMPLETE (<n> case(s) not fully run)`.

The runner appends that output under a final `## Summary` section; `check` ignores the section.
