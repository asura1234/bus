# Test entrypoint

Use `./run test` during development and `./run test --full` for the final gate.
Both execute the same Python scheduler that `just ci` and gate-and-fix call.
Python 3.11+, cargo-nextest, cargo-llvm-cov and the Rust `llvm-tools-preview`
component are the gate prerequisites. Put Cargo's plugin directory on PATH
(usually `$HOME/.cargo/bin`). Actual scheduler and diagnostic invocations create
or update `temp/gate-tools/python` from `scripts/requirements.txt`, then use that
private interpreter. Repeated installs reuse satisfied requirements. Setup uses
an exclusive environment lock and never installs into global Python; failures
identify `temp/gate-tools/python-setup.log` and start no lanes. `--plan` does not
create an environment, install packages, or access the package index.

```sh
./run test --plan                 # explain affected selection without building
./run test --base origin/master   # merge-base plus committed and local changes
./run test --full                 # lint, docs, every applicable test and Rust coverage
```

Both modes read NUL-delimited Git paths, include staged, unstaged and untracked
files, and treat both sides of renames as changes. An invalid base fails before
any test starts, including in full mode. Gate-and-fix passes its resolved
immutable baseline as `--base`; direct invocations default to `origin/master`.
Language eligibility lives in `scripts/suite_lanes.py` and runs before affected
narrowing in `scripts/suite_selection.py`.

The classifier distinguishes first-party Python, Rust, documentation, and shared
inputs. Its path rules, shared-file set, and conservative fallback are executable
policy in that module. Shared inputs enable both languages. Unknown inputs,
unparseable source graphs, and deleted inputs (including documents) whose old
dependency edges are unavailable fail closed. The plan records reasons and prints
`SKIPPED (no Python/Rust changes vs BASE)` for ineligible lanes.

Dependencies override a language skip. Python scripts consumed by Rust keep Rust
tests; production Rust changes keep Python source readers and binary consumers.
A Python-only selection needing Bus builds the CLI as a prerequisite without
enabling Rust test, Clippy, instrumentation, or coverage lanes. A pure Rust test
change skips Python when no Python test reads it. The existing binary-consumer
detector follows source references conservatively and can select extra tests.

Existing docs-only inputs skip both test lanes unless Rust consumes those resources.
Lint and the documentation contracts still run. The documentation contract suite
and transitive Python tests that read changed documents run as documentation
checks, separate from the Python test lane. Binary-dependent doc checks get the
same explicit CLI prerequisite. Directory enumeration retains tests that scan a
changed document's directory, including installed-bundle link checks. Deleted or renamed
inputs fail closed to both languages because their old edges are unavailable.
Imported path constants and reached JSON metadata retain document and source
readers, including packaging NOTICE hash checks; unreadable or invalid reached
metadata fails closed.
Static architecture checks also remain. Lint keeps
formatting, Ruff, architecture, placement, file-length and import checks; Clippy
follows Rust eligibility. The planner supplies its skip to the lint runner.

Within eligible languages, affected ownership remains deliberately narrow:

| Changed paths | Selection |
| --- | --- |
| Known Rust test module with only references inside its component | All binary unit tests in that component's Rust namespace |
| Production Rust source whose graph is certain, in-owner, and not a serde/data shape, CLI surface, or TUI string | That component's binary unit tests, unit tests of transitive referrers, and integration binaries that include the file |
| Integration source/helper | Cargo test targets reachable through source module/path references |
| Dev-tools test source/target | Dev-tools binary tests and CLI/driver integration tests |
| Python with only references inside its owner | That owner's actual `*_test.py` files |
| Cross-owner references, embedded Markdown/data, build inputs, serde/data shapes, CLI surface, TUI strings, uncertain graphs, unknown paths | Every applicable suite |

The index resolves Rust modules and `#[path]` using the existing Rust syntax
helpers, and resolves literal includes relative to the containing file. Python
imports use exact package/source directories and explicit repository `sys.path`
entries, including each existing parent package initializer; path expressions
require a known anchor such as `Path(__file__).parents`.
Temporary fixture paths and matching basenames do not create repository edges.
Rust runtime file literals resolve to exact repository/source paths, including
`concat!(env!("CARGO_MANIFEST_DIR"), ...)` script arguments. Python helper imports
are followed transitively so a script's Rust test consumers also force full fallback.
Source test modules reached transitively from an integration target also fall back
to every applicable suite, covering each crate context that compiles and runs those tests.
The index also follows Python imports when selected tests need `BUS_TEST_BINARY`.
Missing ownership and cross-owner references broaden eligible suites. Embedded
skill resources include directory symlinks, with the collector's test exclusions;
a Cargo rebuild watch alone is not a runtime consumer. A production source file
narrows to its component namespace, transitive test referrers, and integration
binaries that include it. Serde and `src/**/model/**` data shapes, `src/protocol`,
`src/cli`, `src/main.rs`, and `src/terminal` stay on the full suite because the
module graph cannot see those dependents. Direct cross-owner references still
broaden, except an integration test that only includes the production file:
that binary is selected. Extending narrow selection requires reference
evidence and a failing regression test.
An empty diff skips both test lanes; `--full` still runs lint and doc checks.

Affected runs, including conservative fallback, skip the coverage floor and
cannot produce a final merge gate verdict. Full mode runs every test within the
eligible lanes. When Rust is eligible it enforces the unchanged production line
floor after successful tests and nonempty unit profile evidence. When Rust is
skipped, neither instrumentation nor the floor runs; the report records coverage
as SKIPPED. A skipped lane never reports PASS.

Full scheduling overlaps normal-target lint with an instrumented nextest
preflight build. That preflight deliberately selects no tests and builds the CLI
used by Python. Default-target unit/integration builders and CLI/Python consumers
then serialize: preflight does not prevent a later Cargo rebuild or relink.
All test lanes also share one worker budget, including dev-tools on its separate
normal target. Default integration includes the
in-process `server::tests::` harness. Normal-target lint completes before the
dev-tools build; coverage artifacts use cargo-llvm-cov's separate target.
An explicit shared normal/coverage target serializes all binary consumers to
prevent the feature build from replacing binaries while tests use them.
Python-only selections that require the CLI build it first in the configured
target and receive its explicit path, including when no local `target/` exists.
Empty Python owners are skipped; pytest's no-collection exit is accepted while
collection errors and genuine test failures remain failures.
Each gate admits one test lane at a time, with at most **4 direct test workers**.
Rust nextest and libtest batches, including the diagnostic entrypoints, use that
same cap. `NEXTEST_TEST_THREADS` accepts a positive integer capped at 4;
`PYTEST_WORKERS` accepts a nonnegative integer capped at 4, with 0 selecting
serial execution. Single-file Python, lint architecture and the mandatory
documentation contract task run serially. Dependent Python document readers use
the same capped Python pool as other Python tests. Three concurrent gates
therefore admit at most **12 direct test workers**. Controllers, fixture
subprocesses and Cargo compiler jobs are separate
from this direct-worker count. The executable limits and shared scheduler resource
live in `scripts/suite_parallelism.py`; target ownership resources remain in place.
No separate per-run Cargo target is created.

Only the preflight permits an empty nextest selection. Every real Rust batch
fails when it selects no tests. A failed preflight blocks dependent default and
Python tests so they cannot execute a stale CLI; independent dev-tools still
runs. The scheduler collects independent failures, stops children on interruption,
and cleans its owned raw profiles. Live `BUS_*` and `HERDR_*` environment is
removed; Python receives the explicitly built CLI in `BUS_TEST_BINARY`.
Nested execution shares the outer gate's process group ownership; standalone
execution owns its worker groups. Cleanup follows termination of every worker.

Each invocation prints timings and writes complete per-task stdout/stderr and
the head/base/selection/lane statuses to `temp/run-test/run-<id>.json`, following
[the test run format](test-run-format.md). Gate-and-fix wraps the
full entrypoint as `test-full`, alongside `diff-check`, and keeps its clean-tree,
immutable-head checks and encoded artifact logs. The older `just unit-test`,
`integration-test`, `coverage`, and `lint` recipes remain diagnostic tools;
they are not the final gate entrypoint.
