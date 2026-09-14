# Review Plan Guide

This guide contains the workflow principles for `review-plan`. Macro architecture judgment, task-graph safety, finding admissibility, stop rules, and verdicts are defined by [Plan Review Guide](../../docs/guides/plan-review-guide.md). Field names and order are defined by [Review Artifact Format](../../docs/guides/review-format.md).

## Formal review is separate from informal discussion

This skill is for recorded, multi-round review with stable lanes, prior-round reconciliation, independent reviewers, and deterministic artifacts. When the developer casually asks what you think of a plan, discuss it directly after reading the plan and source; do not create round artifacts.

The reviewer is read-only over the plan. Even a one-line correction belongs in a finding's optional remediation, for the author to verify and write. This prevents multiple reviewers from racing to edit the same plan.

## A lane is one independent viewpoint

Use the same `--reviewer <name>` across rounds so a reviewer can reconcile its own history. Parallel reviewers use distinct, stable lanes and do not read one another's `review.md`. Their value comes from independent perspectives; the author merges legitimate overlap by location and semantic root cause.

A bare invocation uses `default` only when unambiguous. Concurrent bare calls may be isolated as `default-2`, `default-3`, and so on. Once multiple or named lanes exist, the exact existing reviewer name must be supplied. Lane identity and `--devils-advocate` posture are independent.

## Lock developer decisions

The goal, non-goals, and archived decisions are review inputs, not invitations to redesign the assignment. A reviewer may report that plan content diverges from those inputs, but may not expand the goal, redefine non-goals, or overturn an archived decision by preference.

The execution workflow is also fixed: one plan is one continuous execution and one PR, with final verification bound to the final candidate. Do not add manual pauses between tasks or waves. A truly multi-purpose plan can be abandoned and split into independent plans; top-level purposes are split, not the execution cadence of one plan.

## Ownership is an exclusive responsibility boundary

Owned files define exclusive task write responsibility and the outer write boundary, not an exact predicted diff allowlist. A stable module or subsystem owner may include descendants not named in the file contract so reasonable tests, helpers, barrels, or lint-driven splits remain possible.

Report isolation problems only for owner overlap, named paths with no owner, boundaries that absorb unrelated responsibility, or ownership that defeats work the plan intentionally separated for safe concurrency. Whether an actual touched file serves the goal is checked through the complete file contract, task goal, deviation report, and task review.

## Mechanical checks and semantic review have separate jobs

The task-graph verifier detects declared cycles, dangling or duplicate IDs/names, owner overlap, globs, missing producer edges, and mechanically parseable path closure. The reviewer does not repeat these findings. The reviewer checks real dependencies the graph omitted, hidden semantic cycles, false edges, missing fallout or sources of truth, wrong task boundaries, and false claims of independent acceptance.

Any cycle means the tasks in that cycle are not independent. Merge them and redraw their external edges. Never hide a cycle with ordering, dynamic edge removal, or forced serialization.

Named changed, added, deleted, and test files belong in the file contract and under exactly one owner. A broad owner cannot replace file-contract or gate coverage. Mechanical import/export edits need a declared authority boundary, replacement rule, affected scope, and verification command—not line-by-line instructions.

## Round 1 establishes coverage; Round 2+ consumes the delta

Round 1 independently reads first-party source and sources of truth before checking the plan. Plan paraphrases are claims, not evidence. Round 1 may use dimension fan-out and fresh eyes, but subagents produce candidates only; the main reviewer verifies, deduplicates, and writes the only `review.md`.

Round 2+ is closed world:

- reconcile unresolved prior findings;
- review the current plan diff;
- inspect contract contradictions or regressions directly introduced by that diff.

Incremental rounds do not explore fresh territory, rescan unchanged text, or promote execution-level implementation detail into a plan finding. Convergence comes from monotonically shrinking scope, not a round cap.

## The triage ledger is author-decision memory

Read every triage ledger oldest to newest; the newest disposition for the same root cause wins.

- `rejected`: do not reopen under a new label unless new evidence disproves the factual premise.
- `applied`: verify the repair; only an incorrect repair creates a carried finding.
- `flagged`: the developer is deciding; do not duplicate it.

This preserves independent reviewer lanes without repeatedly asking the author to adjudicate the same issue.

## Adversarial posture increases proof pressure

`--devils-advocate` means actively seek counterexamples and test unstated assumptions while retaining the same goal, scope, evidence threshold, and readiness rules. Suspicion is not default rejection. A counterclaim needs first-party evidence too. Posture does not create a new lane or turn an incremental round back into a full round.

## A finding is evidence, not persuasion

A finding states location, observation, evidence, impact, and optional remediation. It does not say “blocking,” “recommended,” P0/P1, or name a source model. Only macro problems that can change execution belong in `新问题与建议`. Wording-only drift belongs in `同步清单`. The “other” dimension cannot bypass the macro stop rule; implementation details, style nits, and already-specified control flow are omitted.

The full prior-round reconciliation table remains in `review.md` as audit history, while chat contains only the renderer's summary. `本轮探索区域` is also file-only bookkeeping. All other sections survive unchanged.

## The renderer owns the output protocol

The agent writes a valid, complete `review.md`. `cli_extensions/review_artifact.py render-response` then:

- validates the title, required sections, order, ledger, and verdict;
- removes file-only exploration bookkeeping;
- condenses the prior-round table into its existing summary;
- preserves every other section verbatim.

The final chat response is the renderer output, not a hand-produced summary.
