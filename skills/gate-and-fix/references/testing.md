# Test entrypoint

Use `./run test` during development and `./run test --full` for the final gate.
Both execute the same Python scheduler that `just ci` and gate-and-fix call.
Python 3.11+, pytest, Ruff, cargo-nextest, cargo-llvm-cov and the Rust
`llvm-tools-preview` component are the existing gate prerequisites. Put Cargo's
plugin directory on PATH (usually `$HOME/.cargo/bin`).

```sh
./run test --plan                 # explain affected selection without building
./run test --base origin/master   # merge-base plus committed and local changes
./run test --full                 # lint, every test, fresh coverage floor
```

Affected selection reads NUL-delimited Git paths, includes staged, unstaged and
untracked files, and treats both sides of renames as changes. An invalid base
fails before any test starts. The initial ownership map is deliberately narrow:

| Changed paths | Selection |
| --- | --- |
| Known Rust test module with only references inside its component | All binary unit tests in that component's Rust namespace |
| Integration source/helper | Cargo test targets reachable through source module/path references |
| Dev-tools test source/target | Dev-tools binary tests and CLI/driver integration tests |
| Python with only references inside its owner | That owner's actual `*_test.py` files |
| Cross-owner references, embedded Markdown/data, build inputs, production Rust, unknown paths | Every suite |

The index resolves Rust modules and `#[path]` using the existing Rust syntax
helpers, and resolves literal includes relative to the containing file. Python
imports use exact package/source directories and explicit repository `sys.path`
entries; path expressions require a known anchor such as `Path(__file__).parents`.
Temporary fixture paths and matching basenames do not create repository edges.
Rust runtime file literals resolve to exact repository/source paths, including
`concat!(env!("CARGO_MANIFEST_DIR"), ...)` script arguments. Python helper imports
are followed transitively so a script's Rust test consumers also force full fallback.
Source test modules reached transitively from an integration target also fall back
to every suite, covering each crate context that compiles and runs those tests.
The index also follows Python imports when selected tests need `BUS_TEST_BINARY`.
Missing ownership and cross-owner references fall back to every suite. Embedded
skill resources include directory symlinks, with the collector's test exclusions;
a Cargo rebuild watch alone is not a runtime consumer. Rust is one crate, so production modules do not
receive a guessed unit-only scope. Extending narrow selection requires reference
evidence and a failing regression test.
An empty diff runs no tests; use `--full` when a fresh complete verdict is needed.

Affected runs, including conservative fallback to every suite, skip the coverage
floor and cannot produce a final merge gate verdict. Full mode always runs lint,
all default unit/integration tests, all first-party Python tests, and the
dev-tools CLI/driver and binary unit tests. It enforces the unchanged production
Rust line floor after successful tests and nonempty unit profile evidence.

Full scheduling overlaps normal-target lint with an instrumented nextest
preflight build. That preflight deliberately selects no tests and builds the CLI
used by Python. It then runs default unit tests, default integration tests,
Python, and dev-tools tests concurrently. Default integration includes the
in-process `server::tests::` harness. Normal-target lint completes before the
dev-tools build; coverage artifacts use cargo-llvm-cov's separate target.
An explicit shared normal/coverage target serializes all binary consumers to
prevent the feature build from replacing binaries while tests use them.
Python-only selections that require the CLI build it first in the configured
target and receive its explicit path, including when no local `target/` exists.
Empty Python owners are skipped; pytest's no-collection exit is accepted while
collection errors and genuine test failures remain failures.
Nextest also parallelizes within each batch (four threads by default; override
with `NEXTEST_TEST_THREADS`). No separate per-run Cargo target is created.

Only the preflight permits an empty nextest selection. Every real Rust batch
fails when it selects no tests. A failed preflight blocks dependent default and
Python tests so they cannot execute a stale CLI; independent dev-tools still
runs. The scheduler collects independent failures, stops children on interruption,
and cleans its owned raw profiles. Live `BUS_*` and `HERDR_*` environment is
removed; Python receives the explicitly built CLI in `BUS_TEST_BINARY`.
Nested execution shares the outer gate's process group ownership; standalone
execution owns its worker groups. Cleanup follows termination of every worker.

Each invocation prints timings and writes complete per-task stdout/stderr and
the head/selection/result to `temp/run-test/run-<id>.json`. Gate-and-fix wraps the
full entrypoint as `test-full`, alongside `diff-check`, and keeps its clean-tree,
immutable-head checks and encoded artifact logs. The older `just unit-test`,
`integration-test`, `coverage`, and `lint` recipes remain diagnostic tools;
they are not the final gate entrypoint.
