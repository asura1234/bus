# Gate-and-Fix guide

The goal is to converge every applicable gate failure of one committed diff into a single
partitionable remediation, then prove the complete diff again with a new commit. It does not
replace `rebase-origin-main` or PR creation.

## Repository policy and installed paths

The target is the Git top level of the caller's cwd (or `--repo`); installed support paths
never identify the target. Commands in SKILL.md resolve from the loaded skill directory.
Shared support, when needed, is `Path(__file__).resolve().parents[2] / '_bus'`.
The runner executes the sibling `worktree-new/scripts/repo_profile.py scan --repo PATH --no-cache`
and consumes its stdout, rather than trusting the main-worktree cache across checkouts.
The read-only scan creates no target-tree cache. The schema is owned by worktree-new's `references/repo-profile-format.md`.

`--command` is the caller's selected gate command; when omitted, use `policy.gate_command`.
A missing scanner without an explicit command, or a null/empty command, reports
"no gate configured" with runner exit 2 and no PASS artifact. A failed or malformed
scan is a runner error even if a command was supplied. Never infer Bus by directory name.
Only a profile with `policy.gate_command` equal to `just ci` and the same effective
command enables the Bus adapter below. Other commands run once in the target Git root
through `sh -c` (Windows: `cmd /d /s /c`), plus the diff check, with complete captured logs.
They do not acquire a Bus coverage lifecycle or invoke bus_quality. The configured command
is trusted repository policy; report its actual result without inventing additional gates.
The runner always binds evidence to the clean base/head and rejects concurrent changes,
including for `policy.require_exact_head_gate=true`; false never authorizes stale claims.

## Bus adapter: evidence and boundaries

- The sole source of the diff is the immutable `base...HEAD` resolved when the invocation starts.
  Before starting any gate the runner checks the worktree with
  `git status --porcelain --untracked-files=all`; any tracked or untracked change makes it exit
  with `2` without producing an artifact. The caller must obtain a clean worktree first. Evidence defaults below the absolute Git common
  directory. An explicit unignored source-tree artifact root relocates there; an ignored or external request is honored. The printed path is authoritative.
- After all checks, the runner rechecks the worktree and HEAD before writing the artifact. A
  concurrent edit or commit invalidates the round (`2`, no artifact), even if every command passed.
- Never run the release build (`just build`), the live end-to-end check (`just e2e`), or manual UI
  acceptance. They need packaging or spend model usage, and belong to a dedicated skill or CI. State
  the cost plainly: problems that only those layers expose are no longer caught by this local round;
  when they did not run, report that truthfully, and never stretch this round's PASS into a claim
  that those layers passed.
- Every round runs all four checks plus `git diff --check <base>...HEAD`, regardless of which
  files changed. No changed-file filter can remove a category or a skill test.
  - **Lint**: Cargo fmt, all-target Clippy with warnings denied (for the default build and again
    for the `dev-tools` variant that `./run dev` builds), Ruff `E9,F` over all first-party
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
  - **Environment**: every lane's children run without inherited `BUS_*`/`HERDR_*` variables, so a
    gate started from a Bus agent pane never sees (or writes to) that live Bus. Each Rust test
    binary also drops a live Bus's variables before its tests run (`src/utils/tests/live_bus_env_test.rs`).
  - **Integration**: all Rust integration targets under `tests/`, then the in-process
    `IN_PROCESS_SERVER_TESTS` harness. The same prefix drives nextest and libtest filters.
    Then the dev variant, uninstrumented, with `--features dev-tools`: the `cli` and `tui-driver`
    targets and the `devtools::` unit tests in the `bus` binary, so the dev variant's test tooling
    is built and run; coverage measures only the default build.
    No real LLM agents are launched.
  - **Coverage**: cargo-llvm-cov exports the fresh unit + integration profiles. The Rust production
    line percentage must meet its fixed floor in
    [coverage-policy.json](references/coverage-policy.json). Python has no coverage floor: it is
    developer and agent tooling, not the product, and a percentage target there rewards
    coverage-only tests. Below-floor, missing/empty
    reports, unsuccessful preceding tests, or a different HEAD all fail. Unit starts a new profile
    set, so an old report cannot make a new round pass.
- The runner invokes the same `suite_run.py --full` implementation as `./run test --full`
  and `just ci`, recorded as `test-full` alongside `diff-check`. It requires nextest and
  cannot narrow the final gate through affected selection. Complete task logs and timings
  are retained under `temp/run-test/`; failures also appear in the gate artifact.
  See [the test entrypoint](references/testing.md) for modes and scheduling.
- Coverage reports live in ignored `temp/gate-and-fix/coverage/`. Only the bounded raw-profile
  pools and merged index live in one marked `.gate-profile-runs/run-<id>` directory beneath
  the configured coverage target (by default `target/llvm-cov-target/`). Instrumented build
  products remain in that persistent target for incremental reuse. After export or failure,
  the runner removes its profile directory and report lookup link. SIGINT/SIGTERM stop the
  run's child groups before cleanup; repeated interrupts are deferred through teardown.
  Fresh gates also reclaim valid checkout-owned runs whose PID/start-time owner has died.
  Live and unverifiable owners are preserved. With no live or unverifiable owner, fresh runs
  also clear legacy flat raw profiles and the workspace index before collecting counters.
  Tests run only once per round; coverage consumes
  their profiles before cleanup.
  The gate drops every inherited `BUS_*`/`HERDR_*` variable (see **Environment** above); tests
  that model isolated config roots set and restore those variables within their fixture boundaries.
- Coverage builds use `--no-rustc-wrapper`, so Cargo includes instrumentation flags in its artifact
  cache key even when `CARGO_LLVM_COV_TARGET_DIR` equals `CARGO_TARGET_DIR`. Keep instrumented output
  separate from normal output; regressions cover both separate and shared target settings.
  The unit lane records
  its nonempty `*-unit-*.profraw` files before CLI or integration runs; coverage refuses to export
  if that evidence is missing, deleted, or belongs to another target. It also diagnoses drops over
  20 percentage points from the last successful local report (the policy floor is the reference
  until one exists). Failed measurements never replace that reference. Rerun the complete gate
  after correcting instrumentation; CLI or integration profiles alone cannot waive missing units.
- Raw profile filenames use a four-slot LLVM online-merge pool (`%4m`) per instrumented
  binary and lane, without a per-process `%p` component. The runtime locks each selected
  slot while merging counters, bounding raw-file growth without discarding executions.
  Unit, integration and CLI prefixes stay separate; fresh-round cleanup and the required
  unit-profile checkpoint still prevent stale or CLI-only profiles from passing coverage.
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
- Bus gate argv is owned by `select_gates()` and launches directly; the generic adapter executes
  the repository policy shell command as described above.

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
`just unit-test` starts and records a private run, `just integration-test` continues it, and
`just coverage` consumes its profiles and removes it. A failed lane or interrupted command
removes that run as well; restart from unit after such a failure. Starting unit again discards
its previous inactive owned run. Each run marker records the checkout, owner PID and process start
time. Fresh gates reclaim validated run directories left by dead owners, including PID reuse; live,
foreign or malformed owners are preserved. The instrumented Cargo build cache is persistent. Only
raw profiles and the merged index are run-owned; report lookup links are removed after export and
reclaimed with a dead run. Ownership markers are checked before recursive cleanup; malformed or
foreign metadata is rejected without deleting the referenced files. `just ci` owns the full
sequence in one invocation. Do not run two rounds in the same checkout concurrently.

## Coverage-failure remediation

Apply [code-review-guide "Never pad coverage with a tautology"](../_bus/docs/guides/code-review-guide.md#verification-boundary-prove-behavior-suspicions-with-tests-do-not-run-quality-gates) when
remediating coverage failures. That section owns the ban and the honest remediation alternatives; a
reported gap leaves the gate failed.

## Parallelism and remediation

- The full entrypoint overlaps normal-target lint with the instrumented preflight build.
  Default-target unit/integration builders and CLI/Python consumers then serialize because
  Cargo may rebuild despite preflight. Dev-tools can overlap on its separate normal target;
  explicitly co-located outputs serialize every binary consumer. Normal-target lint
  completes before dev-tools uses that target. Coverage consumes fresh profiles after all
  tasks succeed; the existing nonempty unit-profile checkpoint remains mandatory.
  The independent diff check can run concurrently. All independent failures are collected.
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
