# Gate-and-Fix guide

The goal is to converge every applicable gate failure of one committed diff into a single
partitionable remediation, then prove the complete diff again with a new commit. It does not
replace `rebase-origin-main`, `update-docs`, or PR creation.

## Evidence and boundaries

- The sole source of the diff is the immutable `base...HEAD` resolved when the invocation starts.
  Before starting any gate the runner checks the worktree with
  `git status --porcelain --untracked-files=all`; any tracked or untracked change makes it exit
  with `2` without producing an artifact. The caller must obtain a clean worktree first; an
  in-repository artifact root must be ignored.
- Never run the release build (`just build`), the live end-to-end check (`just e2e`), or manual UI
  acceptance. They need packaging or spend model usage, and belong to a dedicated skill or CI. State
  the cost plainly: problems that only those layers expose are no longer caught by this local round;
  when they did not run, report that truthfully, and never stretch this round's PASS into a claim
  that those layers passed.
- The Bus gate set is derived by the runner from the committed diff; an agent cannot guess or
  shrink it. Every round always selects one of:
  - `just ci`, when both `just` and `cargo-nextest` are available on the host;
  - otherwise its direct expansion: `cargo fmt --check`,
    `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, the
    `scripts.test_*` maintenance unittest set, `scripts.test_ui_hot_path_architecture`, the Bun
    integration-asset tests, and the `workers/plugin-marketplace` install and tests;
  - followed by `git diff --check <base>...HEAD`.
  The artifact records the exact commands that ran, never claiming that an unavailable wrapper ran.
- Then, by changed path, append `skill-tests` (`python3 -m pytest -q <files>`):
  - `skills/<name>/**` → the pytest files owned by that skill
  - `cli_extensions/**`, `docs/guides/**` or `docs/templates/**` → every skill-owned pytest file
- `just ci` does not discover skill-local pytest files, so this lane is how a skill change gets its
  own tests run. Shared contract inputs select every skill test because skill tests read
  `cli_extensions/**`, `docs/guides/**` and `docs/templates/**` as contracts, and those paths have
  no finer, provable owner.
- Selection follows the complete PR diff and never shrinks because only some owner failed in the
  previous round. A local PASS does not claim the whole tree is green; the full matrix is owned by
  GitHub CI.
- Format and lint run over the whole Cargo workspace: neither `cargo fmt --check` nor
  `cargo clippy` takes a changed-file scope, so the skill does not rebuild a file-to-language
  classification.
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

## Parallelism and remediation

- The runner starts non-conflicting gates concurrently, capped at `MAX_PARALLEL_GATES`; gates that
  declare the same writable resource are scheduled mutually exclusively, and a gate that declares
  exclusive execution waits for in-flight gates to exit before starting, and no other gate starts
  while it runs. `ci` and every gate of its direct expansion declare exclusive execution, so they
  run one at a time in selection order; `skill-tests` and
  `diff-check` declare neither resources nor exclusivity and run within the concurrency cap. This
  changes neither test selection nor pass criteria.
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
