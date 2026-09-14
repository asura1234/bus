# Review PR Guide

This guide contains the workflow principles for `review-pr`. Code-review dimensions, severity semantics, architecture judgment, and nit boundaries are defined by [Code Review Guide](../../docs/guides/code-review-guide.md). Field names and order are defined by [Review Artifact Format](../../docs/guides/review-format.md).

## A lane is one independent viewpoint

Use the same stable reviewer lane across rounds so a reviewer can reconcile its own history. Parallel reviewers use distinct lanes and do not read one another's `review.md`. Their value comes from independent perspectives, not identical reports; the author merges legitimate overlap by location and semantic root cause.

A bare invocation uses `default` only when unambiguous. Concurrent bare calls may be isolated as `default-2`, `default-3`, and so on. Once multiple or named lanes exist, the exact existing reviewer name must be supplied.

## Lock the goal; do not change the assignment

With an associated plan, the goal, non-goals, and archived decisions are review inputs, not redesign prompts. Without a plan, the goal must come from the developer's one-sentence answer and cannot be inferred from the diff, commits, or PR body.

A reviewer may report code that diverges from those inputs, but may not expand the goal, move toward a non-goal, or replace an archived choice by preference. Only new source evidence that disproves a decision's factual premise may be escalated to the developer.

Apply the code-review guide's goal-relevance gate before investigation, proof, delegation, and output. Dimension 6 is the only exception: classify out-of-goal diff by cumulative slice size without reviewing its implementation. `XS`/`S` becomes non-blocking consistency drift; `M` or larger becomes a substantive scope finding that may recommend `split-pr`. Permission to include incidental work does not create an obligation to repair it. Diff and round boundaries may narrow scope further.

## The plan is input, not a code-review target

Committed plan-document changes are excluded from code-review scope. They do not enter diff snapshots, delta, touched-file sets, findings, consistency drift, or single-purpose calculations. An explicitly associated plan supplies only locked goal, non-goals, and archived decisions. When no other committed changes remain, the prologue fails because there is no code to review.

## Round 1 establishes coverage; Round 2+ consumes the delta

Round 1 first partitions the diff by goal relevance, reads every eligible touched file completely, covers all nine dimensions, and records any incomplete region. Upstream and downstream reading serves the goal-related judgment and does not become a general module audit.

Round 2+ is closed world:

- recheck goal relevance and reconcile open prior findings;
- identify and review goal-related changes in the current delta;
- inspect contract contradictions or regressions directly introduced by that delta;
- cover a missed Round 1 region only when the prior ledger explicitly proves it was missed.

Do not rescan covered unchanged code or use fresh territory to maintain a finding count. Convergence comes from monotonically shrinking scope, not a round limit.

## Prove behavioral suspicion with a test first

Test proof applies to runtime-behavior claims in correctness, security, and bug dimensions. Other dimensions are established by reading source and cite path:line observations; they do not create probes or use proven/unproven labels.

In the behavioral dimensions, writing a test is the default action. First apply the goal-relevance gate, then turn the suspicion into a focused regression case. A red test provides root cause, reproduction, and repair acceptance. A suspicion that cannot be expressed as a test is usually not understood well enough to report.

Write access is limited to test files: create a test file or add a new case to an existing one. Never edit implementation, configuration, build scripts, or docs, and never modify code to manufacture a red result. A probe must have a real failure mode; assertions about the test harness or a test-configured mock prove nothing.

Run the narrowest relevant test target or exact test name. Related existing tests may also run. Full lint, formatting, coverage, build, full-suite tests, and E2E belong to CI and `gate-and-fix`.

Probe tests remain in the worktree, uncommitted, for the author to adopt, rewrite, or remove. They should be normal durable regression tests with semantic names. Never put lane, dimension, or round identifiers in test names, delete another reviewer's probe, or weaken existing assertions.

A red probe becomes a finding whose evidence begins `已证明：<test @ relative path> — <failed assertion>`. A green probe withdraws the suspicion and is recorded only in file-only exploration bookkeeping.

If behavior can be proven with Unit or Integration tests, prove it before reporting. If only a real terminal session can confirm it, the evidence may begin `未证明`; state that the behavior cannot be ruled out and write impact conditionally. An unproven suspicion does not block Ready, trigger another round, or independently justify changing manually verified behavior.

## The triage ledger is author-decision memory

Read every triage ledger oldest to newest; the newest disposition for the same root cause wins.

- `rejected`: do not reopen under a new label unless new evidence disproves the factual premise.
- `applied`: recheck goal relevance and verify the repair; prior adoption never expands the goal.
- `flagged`: the developer is deciding; do not duplicate it.

## Fan-out increases coverage, not report count

Subagents are narrow partitions inside one reviewer lane, not extra reviewers. Use the fewest dimension groups justified by the diff. Small diffs stay serial.

Subagents return raw candidates only. The main reviewer verifies them against current source, rechecks goal relationship, deduplicates them, removes nits, and writes the only `review.md`. Close all subagents before reporting. Fresh-eyes and platform-native review are optional full-round candidate sources and never reopen incremental scope.

## Adversarial posture increases proof pressure

`--devils-advocate` means actively seek counterexamples and test unstated assumptions while retaining the same goal, scope, evidence standard, and readiness rules. Suspicion is not default rejection; a counterclaim requires evidence too. Posture does not create a new lane or turn an incremental round into a full one.

## A finding is evidence, not persuasion

Reports are agent-anonymous. A finding states location, observation, evidence, impact, and optional remediation. It does not say “blocking,” “recommended,” P0/P1, or name a source model. Severity influences only the single final verdict.

Do not narrate exploration, praise the implementation, write first-person retrospectives, or ask what to do next. With no substantive problem, `新问题与建议` contains exactly `无。`. Rejected candidates may appear only in file-only exploration bookkeeping.

Consistency drift normally means wording. Dimension 6 `XS`/`S` out-of-goal code slices are the explicit exception. The “other” dimension still requires goal relevance and materiality and cannot collect implementation detail or style nits.

The full prior-round reconciliation table remains in `review.md` as audit history, while chat contains only the renderer's summary. `本轮探索区域` is also file-only bookkeeping. All other sections survive unchanged.

## The renderer owns the output protocol

The agent writes a valid, complete `review.md`. `cli_extensions/review_artifact.py render-response` then:

- validates the title, required sections, order, ledger, and verdict;
- removes file-only exploration bookkeeping;
- condenses the prior-round table into its existing summary;
- preserves every other section verbatim.

The final chat response is the renderer output, not a hand-produced summary.
