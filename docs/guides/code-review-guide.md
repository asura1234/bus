# Code Review Guide

Code review evaluates the committed implementation against one locked purpose. Findings must identify concrete, reachable consequences supported by current source or focused proof. The review is not a general cleanup pass over nearby code.

## Review context: existing behavior is a verified baseline

PR review normally follows implementation validation. Existing runtime behavior is therefore treated as a manually or mechanically verified baseline unless positive evidence shows that a reachable input produces a wrong result. A true suspicion does not automatically authorize a risky behavioral rewrite. Preserve verified behavior, strengthen it with a regression test when appropriate, and escalate genuine product or architecture choices to the developer.

Task review during plan execution does not inherit this assumption; task work has not yet reached final validation.

## Review scope

Review the committed diff against the explicit base, the complete current contents of eligible touched files, direct callers and consumers needed to reason about those changes, relevant tests, and applicable architecture sources of truth. Exclude uncommitted user work and plan-document changes from PR findings.

Round 1 establishes complete goal-related coverage. Round 2+ is closed world: prior findings, the new committed delta, explicitly recorded Round 1 coverage gaps, and direct consequences only. A finding fixed at current HEAD is resolved even when the historical comment was once correct.

Do not turn code review into exhaustive QA of unrelated states or into enforcement of a plan checklist. The locked goal defines review relevance.

## Goal-relevance admission

Before investigating, proving, delegating, or reporting a candidate, establish:

1. which part of the locked goal it serves;
2. the reachable behavior or repository-safety consequence;
3. whether the requested correction stays inside that goal and outside stated non-goals.

Without this link, stop. Do not write a probe or inspect the incidental subsystem deeply.

Dimension 6 is the sole exception. Every out-of-goal committed slice must be classified by its distinct purpose and cumulative review size. `XS`/`S` slices enter the non-blocking consistency list. `M` or larger slices become a substantive scope finding that may recommend `split-pr`. This classification does not authorize implementation review or remediation of the secondary purpose.

The developer alone may change the goal or non-goals. A review finding cannot do so.

## Three verdicts

| Verdict | Meaning |
| --- | --- |
| `Ready` | Goal-related review coverage is complete, no admitted SUBSTANTIVE finding blocks readiness, and all prior findings are validly closed. |
| `Needs Refinement` | The change is locally salvageable but one or more admitted SUBSTANTIVE findings remain, including an out-of-goal `M`+ slice. |
| `Abandon` | The architecture is fundamentally wrong or the change cannot be repaired within the current diff. |

`Ready` requires coverage, not merely zero findings. `satisfactory` means a root cause is repaired; evidence-backed `rejected` or `withdrawn` also validly closes a claim that should not be fixed. Unproven behavior suspicion and consistency-only drift do not block Ready.

`Abandon` means “not locally salvageable,” never “too many rounds.” Any locally repairable SUBSTANTIVE issue remains `Needs Refinement` regardless of round count. Dimension 6 alone never yields `Abandon`.

No other review verdict is valid.

## Convergence: SUBSTANTIVE versus CONSISTENCY

SUBSTANTIVE findings can change runtime correctness, safety, architecture, compatibility, purpose, or test validity. CONSISTENCY drift is wording, naming, comments, documentation, or an `XS`/`S` out-of-goal scope slice that does not change delivery. Only SUBSTANTIVE findings trigger another round.

Incremental review accepts a new finding only when it comes from:

1. an unresolved prior finding;
2. the current delta;
3. a contract contradiction or regression directly introduced by that delta;
4. an explicit Round 1 coverage gap recorded in the prior artifact.

Do not rescan unchanged code, invent fresh scope, or reopen a rejected root cause under new wording.

## Clean Architecture principles

### Separation of concerns

Keep reusable domain behavior independent of terminal rendering and process adapters. State owners coordinate lifecycle and transitions. Views render and forward intent. Infrastructure provides terminal, process, persistence, and transport capabilities behind explicit interfaces.

### Single responsibility

A function, type, file, or module should have one coherent reason to change. Split code that combines unrelated state ownership, protocol parsing, rendering, and persistence.

### Minimal public API

Public Rust items, module exports, and command/protocol surfaces are durable contracts. Expose only symbols required by current external consumers; prefer private or crate-local visibility otherwise.

### Dependency direction

Presentation may depend on domain contracts, but domain behavior must not depend on terminal views. Concrete adapters implement interfaces owned by policy layers. Verify directions against module `AGENTS.md` and architecture sources of truth.

### Appropriate dependency injection

Inject a dependency when lifecycle must be shared, the implementation varies by platform/environment, or tests require substitution. Do not add a registry or abstraction for a single fixed use.

### Interface segregation

Prefer narrow traits aligned with real callers. Do not create broad interfaces whose methods serve unrelated consumers.

### Module communication

Use explicit data types, events, and public methods. Do not reach into another module's internal mutable state.

### Defensive checks belong to the behavior owner

Validation and invariant enforcement belong with the owner of the underlying state or domain rule, not duplicated in each caller or view.

### Fail fast at the nearest state owner

An invariant violation should fail at the closest layer that owns the invariant, where context is intact. Avoid carrying impossible state deeper into the system and converting it into misleading fallback behavior.

### Do not add fallback for impossible or unrecoverable failures

Fallback is useful only for reachable failures with meaningful degradation. Remove catches, placeholders, silent retries, and default values that hide app-fatal dependencies or impossible states. This rule does not weaken handling for real recoverable I/O, process, protocol, or persistence failures.

### KISS

Prefer the smallest implementation that satisfies current requirements and protects real invariants. New wrapper types, strategy layers, locator objects, registries, and generic frameworks require current evidence that they reduce complexity or represent a stable shared contract.

### DRY

Reuse or evolve the existing canonical owner when old and new behavior share purpose, semantics, invariants, and reasons to change. Do not keep a dedicated implementation and add a second generic copy. Do not merge code that is only superficially similar.

### YAGNI

Do not add configuration, extension points, states, or failure paths for imagined future needs.

### Follow existing patterns

Adjacent repository patterns are the default. A new pattern requires evidence that the established pattern cannot satisfy the locked goal.

### Complete refactors in one step

When the goal is a refactor, move every in-scope caller and remove the old path in the same change unless compatibility is itself an explicit current requirement.

### No meaningless deprecation periods

Do not preserve an unused compatibility shim or dual path merely to avoid deleting code. A transition period needs a real external consumer and an explicit removal contract.

## Review dimensions

### 1. Architecture and design

Check responsibility placement, dependency direction, lifecycle ownership, public API, duplicate mechanisms, state authority, and compatibility with architecture sources of truth. Report only consequences tied to the locked goal.

### 2. Correctness

Check reachable inputs, boundary values, state transitions, error propagation, cancellation, ordering, persistence, and protocol contracts. A behavioral suspicion should become a focused test and run red before becoming a finding whenever Unit or Integration proof is possible.

### 3. Security

Check authorization, trust boundaries, injection, path handling, untrusted terminal/process input, credential or secret exposure, and fail-open behavior. Security behavior that is testable must be proven with a focused red test.

### 4. Log coverage

Require actionable logs at boundaries where failures otherwise become invisible: process launch, persistence, protocol parse, callback delivery, external I/O, and recovery. Avoid duplicate logs at every layer, sensitive values, or logs on hot paths without rate control. Logging gaps must have a concrete diagnostic consequence.

### 5. Naming and comments

Names must accurately describe responsibility, units, ownership, and lifecycle. Comments explain non-obvious invariants or reasons, not restate code. Pure style belongs outside findings; misleading names or comments become findings only when they can cause incorrect use or maintenance.

### 6. PR single purpose

Partition committed changes by the locked goal. Group out-of-goal changes by actual secondary purpose and size the cumulative slice using review complexity, not only raw lines. Put `XS`/`S` in the consistency list and `M`+ in substantive findings with `split-pr` as a possible remediation. Do not review the secondary implementation in other dimensions. Scope drift alone never yields `Abandon`.

### 7. Bugs and unanticipated edge cases

Trace reachable edge conditions the change can encounter: empty data, repeated events, cancellation, shutdown, stale callbacks, partial writes, terminal capability differences, concurrent ownership, corrupted state, and boundary timing. Convert testable suspicions into focused failing tests before reporting. Do not enumerate impossible state combinations.

### 8. Over-engineering and branch proliferation

Report abstractions, fallbacks, feature switches, parallel implementations, or state branches that are unnecessary for the locked goal and create real maintenance or correctness cost. Do not label a design over-engineered merely because it differs from personal taste.

### 9. Other

Use only for a material, goal-related defect that fits no earlier dimension. It cannot bypass goal relevance, proof standards, or the nit boundary.

## Guardrails

1. Read complete eligible touched files, not isolated hunks.
2. Verify behavior and ownership from current source; names and comments are claims, not proof.
3. Trace direct callers and consumers only as needed for the changed contract.
4. Prefer reachable failure paths over hypothetical misuse.
5. Preserve verified behavior unless positive evidence proves a wrong result.
6. Never treat a green unrelated test or zero selected tests as evidence.
7. Keep reviewer lanes independent; do not read another lane's artifact.
8. Do not expand review into unrelated pre-existing debt.
9. Do not turn style preferences, speculative hardening, or cleanup into findings.

## Adversarial posture: skeptical, not contemptuous

Adversarial review seeks the strongest counterexample and proves it. It does not replace the author's unsupported story with the reviewer's unsupported story. Challenge universal claims, hidden lifecycle assumptions, omitted failure paths, weak tests, unnecessary abstractions, and architecture drift while applying the same goal and evidence standards.

## Verification boundary: prove behavior, do not run quality gates

### Do

- create or extend a focused test for a behavioral suspicion in dimensions 2, 3, or 7;
- run the narrowest relevant Cargo test target or exact test name;
- inspect relevant existing tests;
- keep probes semantic and suitable for permanent adoption.

### Do not

- modify production code, configuration, build scripts, docs, or plans;
- alter implementation to manufacture a failure;
- run formatters, fixers, full lint, coverage, builds, full suites, or E2E;
- delete or weaken existing assertions;
- commit or push reviewer tests.

### Probe naming and ownership

Assign non-overlapping test files before parallel review. Prefer normal behavior names; never encode reviewer, lane, dimension, or round. A reviewer may add a case to an existing test file but cannot overwrite another probe.

### Probe lifecycle

- Red: keep the test, write the finding, and start evidence with `已证明` plus the test and failed assertion.
- Green: withdraw the suspicion, keep the test for the author, and record it only in file-only exploration bookkeeping.
- Never leave a red probe without a finding.

### Unproven suspicion

When only a real terminal/manual session can establish a behavior, evidence may begin exactly `未证明`. State “cannot rule out” and write impact conditionally. Unproven suspicion does not block Ready or trigger another round.

### Evidence recording

Behavioral findings cite the probe and failure first, then the implementation path that explains it. Source-only dimensions cite precise path:line evidence and a concrete consequence. A plan, commit message, or previous review is not first-party proof of current behavior.

## Nits versus findings

A finding must be material, actionable, goal-related, and evidence-backed. It changes a user-visible result, repository safety, architecture contract, compatibility, or the ability to validate the change. Formatting preference, alternative naming without misuse risk, speculative defensive coding, generalized cleanup, and unrelated pre-existing debt are omitted.

## Output format

Use [Review Artifact Format](review-format.md). Findings contain no severity or disposition labels. The artifact is agent-anonymous.

### Detailed analysis

Record exact location, dimension, observation, evidence, impact, and optional remediation. Put wording-only drift and dimension 6 `XS`/`S` slices in the consistency list.

### Final verdict

The final verdict is exactly one of:

- `Ready`
- `Needs Refinement`
- `Abandon`

It is the final line of the rendered response after all findings. Do not emit a conditional verdict or multiple verdicts.

## Additional resources

Use repository `AGENTS.md`, module sources of truth, tests, and official platform/library documentation as first-party context. External best-practice articles do not override repository contracts or reachable source behavior.
