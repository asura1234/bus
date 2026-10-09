# Gate-and-Fix guide

The goal is to converge every applicable gate failure of one committed diff into a single
partitionable remediation, then prove the complete diff again with a new commit. It does not
replace `rebase-origin-main` or PR creation.

## Evidence and boundaries

- The sole source of the diff is the immutable `base...HEAD` resolved when the invocation starts.
  Before starting any gate the runner checks the worktree with
  `git status --porcelain --untracked-files=all`; any tracked or untracked change makes it exit
  with `2` without producing an artifact. The caller must obtain a clean worktree first; an
  in-repository artifact root must be ignored.
- After all checks, the runner rechecks the worktree and HEAD before writing the artifact. A
  concurrent edit or commit invalidates the round (`2`, no artifact), even if every command passed.
- Never run the release build (`just build`), the live end-to-end check (`just e2e`), or manual UI
  acceptance. They need packaging or spend model usage, and belong to a dedicated skill or CI. State
  the cost plainly: problems that only those layers expose are no longer caught by this local round;
  when they did not run, report that truthfully, and never stretch this round's PASS into a claim
  that those layers passed.
- Every round runs all four checks plus `git diff --check <base>...HEAD`, regardless of which
  files changed. No changed-file filter can remove a category or a skill test.
  - **Lint**: Cargo fmt, all-target Clippy with warnings denied, Ruff `E9,F` over all first-party
    Python, and the static hot-path architecture contract. The lint lane runs the hot-path and
    import-boundary pytest suites under `tools/tests/`, then checks test placement and file sizes before enforcing the final import graph.
    Unknown source/target owners and forbidden component or inner edges fail the lane. These checks read source text;
    they are not UI tests. Rust formatting and Python syntax/pyflakes violations fail the gate.
    Python style-only rules and LibTV's TypeScript-specific complexity limits are not imported. All
    first-party handwritten production Rust/Python files have an 800-line cap. Comments and blank lines count;
    shared `*_test.*` / `tests` paths and Rust `cfg(test)` scopes are exempt. Generated Ghostty FFI declarations are
    separately named in [lint-policy.toml](references/lint-policy.toml); no handwritten per-file exemptions remain.
    Production Clippy runs first with function length 100, cognitive complexity 25 and argument threshold 11,
    plus wildcard imports, stdout print macros, dbg/todo/unimplemented, get-unwrap and unwrap denied. All-target
    Clippy follows with only those selected rules allowed for test compilation; other default warnings still fail.
    `just windows-lint` uses the same production/test policy on the Windows target.
  - **Unit**: instrumented Bus binary tests, excluding the `IN_PROCESS_SERVER_TESTS` prefix in
    `bus_quality.py`, followed by every `test_*.py` / `*_test.py` in its existing `PYTHON_ROOTS`
    (`scripts/`, `skills/`, `cli_extensions/`, `tools/`, `packaging/`) with pytest. They must pass;
    their coverage is not measured.
    This includes all maintenance suites and all skill suites, on every round.
    The instrumented CLI is built before Python tests; its explicit `BUS_TEST_BINARY` and profile
    path ensure CLI tests measure this commit rather than an old `target/debug/bus`.
  - **Integration**: all Rust integration targets under `tests/`, then the in-process
    `IN_PROCESS_SERVER_TESTS` harness. The same prefix drives nextest and libtest filters.
    No real LLM agents are launched.
  - **Coverage**: cargo-llvm-cov exports the fresh unit + integration profiles. The Rust production
    line percentage must meet its fixed floor in
    [coverage-policy.json](references/coverage-policy.json). Python has no coverage floor: it is
    developer and agent tooling, not the product, and a percentage target there rewards
    coverage-only tests. Below-floor, missing/empty
    reports, unsuccessful preceding tests, or a different HEAD all fail. Unit starts a new profile
    set, so an old report cannot make a new round pass.
- When both `just` and `cargo-nextest` exist, the runner invokes `just lint`, `just unit-test`,
  `just integration-test`, and `just coverage` separately so each check has its own complete log.
  Otherwise it directly invokes the identical `bus_quality.py` implementations. Rust collection
  uses nextest when available and `cargo llvm-cov test` otherwise. `just ci` runs the same four
  recipes; it accepts no filter that could shrink the mandatory set. The artifact records the exact
  wrapper argv and its logs record every underlying command.
- Coverage reports live in ignored `temp/gate-and-fix/coverage/`; instrumented Cargo output stays
  in `target/llvm-cov-target/`. Tests run only once per round; coverage consumes their profiles.
  The gate retains the parent `BUS_DATA_DIR`, `BUS_SESSION_ID`, and the native-session
  compatibility key `HERDR_SESSION`; tests that
  model isolated config roots clear and restore those variables within their fixture boundaries.
- Rust coverage includes host-compiled first-party `src/` executable lines. Vendor/dependencies,
  build.rs, generated files from `lint-policy.toml`'s `generated_files`, `tests/` and inline
  `#[cfg(test)] mod` bodies are excluded. Host-inactive platform code is outside LLVM's inventory.
- Restructure path inputs are grouped at the top of `bus_quality.py`. Missing Python roots are
  skipped by discovery and Ruff. `GENERATED_RUST` reads the lint policy and drives the coverage regex, so a
  generated-file move requires only a policy edit. Rust source roots and architecture check paths
  are listed in the same block.
- The Rust floor is **80%** aggregate production line coverage (measured around 84% when it was
  set), not per-file or branch coverage. Review any future reduction as a policy change; never
  lower it to clear a failure.
- The reference rule is LibTV App's strict lint failures and explicit coverage inventory, plus
  LibTV Desktop's fail-closed native LLVM reports and individually justified exclusions. Bus uses
  a measured aggregate line baseline, rather than their TypeScript per-file 100% rule.
- The runner's `0` means only a PASS artifact, `1` means only a FAIL artifact, and `2` means a
  runner error with no consumable artifact. Read the single artifact path it prints only on
  `0`/`1`; stop immediately on any other exit code.
- Every invocation produces a new artifact; old artifacts are for audit only and must never be
  treated as this round's result. Before reading, validate format and base identity with the
  runner's `verify` subcommand.
- The artifact encodes each stdout/stderr as base64 UTF-8 plus its byte count; a consumer first
  obtains the failed gate names through the runner's `list` subcommand after base validation, then
  uses `show` to fetch one complete log stream of a gate, which is the only way to restore empty
  output, a final newline, and trailing whitespace verbatim.
- Every gate argv is owned solely by the runner's `select_gates()`; it launches each entry directly,
  never through a shell.

## Developer tools

No Cargo.toml dependency is added. Python 3.11+ is required. Install developer tools once:

```sh
cargo install cargo-llvm-cov --locked
rustup component add llvm-tools-preview
python3 -m venv --system-site-packages temp/gate-tools/python
temp/gate-tools/python/bin/python -m pip install -r skills/gate-and-fix/scripts/requirements.txt
# Optional wrappers/faster Rust execution: install just and cargo-nextest.
```

On Windows, use `python` in place of `python3` and invoke the runner with the venv's
`Scripts/python.exe` (or pass that interpreter with `just --set python <path> ci`).
The runner and just use the local venv if present; otherwise they use the current/default Python,
which must have the requirements installed. Missing mandatory tools fail with setup guidance.
`just coverage` consumes the immediately preceding `just unit-test` and `just integration-test`;
use `just ci` for the full sequence. Do not run two rounds in the same checkout concurrently.

## Parallelism and remediation

- The four checks declare exclusive execution and run in lint → unit → integration → coverage
  order because they share Cargo output and coverage profiles. The independent diff check can run
  within the concurrency cap. All check results are collected even if an earlier check fails;
  coverage fails closed when a prerequisite failed.
- A FAIL round must first collect every gate result and group them by writable owner (not by the
  lint/test category). **How to fix after that is orchestrated by main**: fix sequentially itself,
  dispatch subagents by owner, or mix the two, depending on the size of the failure surface,
  whether the owners are genuinely disjoint, and how much context is left. The shape is
  deliberately not fixed here — unlike the per-claim fan-out removed from the author side, the
  remediation itself is **mutually disjoint** work (allowlists by definition never overlap), so
  dispatching never rebuilds the same context twice, and whether it pays off depends only on
  whether each group's work outweighs one brief; one or two small fixes are faster done directly.
  When dispatched, each subagent changes only its own allowlist and never touches Git.
- An unclear owner, overlapping allowlists, a blocker (whether main itself or a subagent hit it), or
  no intended remediation delta is a STOP. Do not mask it with a retry in the next round, a guessed
  owner, or edits to unrelated dirty files.
- Only after every owner is collected and the main agent has verified the combined delta does it
  run `commit-and-push` once. After every commit, rerun the complete applicable gate set; never
  rerun only the subset covering the fixed files.

## Non-goals

- No separate lint-remediation subflow; format/lint failures, like test failures, are artifact
  evidence handed to their owner to fix.
- No documentation updates, no PR creation or editing, no rebase; the caller decides on those after
  the gate loop succeeds.
