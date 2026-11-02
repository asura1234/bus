# Test run report format

`scripts/suite_run.py` writes one UTF-8 JSON report per invocation under
`temp/run-test/run-<unique id>.json`. Plan mode prints the selection, `head`,
`base`, changed `paths`, `lanes`, and `direct_test_worker_limit` without executing commands.

A completed report contains:

| Field | Value |
| --- | --- |
| `head` | Git commit SHA tested |
| `base` | Comparison ref; gate-and-fix supplies its resolved SHA |
| `selection` | Planner dataclass: full breadth, Rust filter, Python roots/files, doc consumer tests, dev-tools, binary prerequisite, reasons, coverage eligibility |
| `lanes` | Exactly `python` and `rust`, each with `status` and `reason` |
| `coverage` | `status` and `reason` for the Rust floor |
| `exit_code` | Integer process outcome |
| `duration_ms` | Nonnegative integer wall time |
| `coverage_ms` | Nonnegative integer, or `null` when coverage did not execute |
| `direct_test_worker_limit` | Maximum direct test workers across concurrent test lanes; currently `4` |
| `results` | Ordered command results: `name`, string-list `argv`, integer or null `exit_code`, nonnegative `duration_ms`, complete `stdout` and `stderr` |

Plan lane statuses are `RUN` or `SKIPPED`. Completed lane statuses are `PASS`,
`FAIL`, `NOT_RUN` (no command result, including blocked prerequisites), or
`SKIPPED`. Ineligible lanes retain `SKIPPED` and reason
`no Python changes vs BASE` or `no Rust changes vs BASE`; they never become PASS.
Coverage statuses are `PASS`, `FAIL`, or `SKIPPED`. A Rust skip gives coverage
the same reason. Affected mode gives coverage reason `affected mode`.

For example, a docs-only full run has:

```json
{
  "lanes": {
    "python": {"status": "SKIPPED", "reason": "no Python changes vs baseline"},
    "rust": {"status": "SKIPPED", "reason": "no Rust changes vs baseline"}
  },
  "coverage": {"status": "SKIPPED", "reason": "no Rust changes vs baseline"},
  "coverage_ms": null
}
```

The example is a fragment; an actual report requires every field in the table.
Missing or unknown statuses, and representing a skipped floor as PASS, are
invalid. Gate-and-fix retains its separate schema-3 round artifact: `test-full`
is the aggregate command result, whose captured stdout names skips and links
the detailed JSON report.
