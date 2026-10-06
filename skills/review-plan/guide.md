# Review Plan Guide

This guide holds the workflow principles of `/review-plan`. The architecture judgment, task-graph safety, admissibility,
stop rule, and verdict of plan review are all defined by the
[Plan Review Guide](../../docs/guides/plan-review-guide.md); report fields and order are all defined by the
[Review Format](../../docs/guides/review-format.md).

## Keep formal review separate from free discussion

This skill serves formal multi-round review that must leave a record: stable lanes, prior-round reconciliation, independent viewpoints from multiple reviewers, and
deterministic artifacts. When the developer just casually asks "what do you think of this plan", you may read the plan and source per the Plan Review Guide and
discuss it directly, without creating round artifacts.

The reviewer is read-only over the plan. Even when a repair only changes one sentence, it may only be given as exact
replacement text in the finding's "possible remediation", for the plan author to verify and write. This keeps multiple reviewers from racing to write the same plan.

## A lane is one independent viewpoint

The same reviewer uses a stable `--reviewer <name>` across rounds so it can reconcile its own history. When multiple models review in parallel,
each reviewer uses a distinct, stable lane, and none reads another's `review.md`. The value of different models comes from independent
viewpoints; reports are not required to restate one another; the author merges legitimate overlap by location and semantic root cause.

When unambiguous, a bare invocation uses the `default` lane. Concurrent bare invocations may be temporarily isolated by the script into `default-2`, `default-3`;
once multiple or named lanes exist, the same reviewer name listed by the script must be passed back explicitly. The lane and
the `--devils-advocate` posture are independent of each other.

## Lock the choices the developer has already made

`## 目标`, `## 非目标`, and `## 已归档的决策` are review inputs, not content for the reviewer to redesign.
The reviewer may report plan content that diverges from the goal, runs into a non-goal, or violates an archived decision, but may not expand the goal, redefine
non-goals, or overturn an archived decision by personal preference.

The plan workflow is a fixed input too: one plan is one one-shot execution and one PR, and the final e2e binds only to the candidate after
all tasks and the EXIT CHECK. Do not turn tasks or waves into manual pause points. A genuinely multi-purpose plan
may be judged Abandon and split into multiple independent plans; what gets split is the top-level purpose, not the execution cadence of one plan.

## Ownership is an exclusive responsibility boundary

`拥有文件` is the exclusive write boundary between tasks and the outermost write ceiling, not an exact allowlist of the predicted diff.
A stable module/subsystem owner may cover descendants not named in the file contract, to accommodate new tests, helpers,
barrels, lint-driven file splits, and other reasonable deviations.

Report an isolation problem only when owners overlap, an explicit touch point has no owner, a boundary swallows unrelated responsibility areas, or ownership breaks tasks
that could otherwise run safely in parallel. Do not demand file-level minimization merely because a directory owner is larger than the predicted touch points. Whether the actual touched files
serve the goal is judged jointly by the file contract, task goal, deviation report, and task acceptance during execution.

## Mechanical checks and semantic review divide the work

`verify_task_graph.py` handles declared cycles, dangling or duplicate ids/names, owner overlap, globs, missing consumes edges,
and parseable explicit path closure. The reviewer does not copy mechanical findings; it checks only what the script cannot prove: real dependencies,
hidden semantic cycles, false edges, missing fallout/SOT, wrong task boundaries, and false independent acceptance.

Any cycle means the tasks inside it are not independent units; the only repair direction is to merge the tasks in the cycle and redraw the outward edges. Never use ordering,
dynamic edge removal, or forced serialization to mask a cycle.

Files and concrete tests that the plan explicitly names must enter the file contract and be covered by one and only one owner. An owner may be wider than
the file list, but an explicit path cannot be ownerless; when a public contract, module structure, or dependency edge changes, the corresponding architecture /
AGENTS and other SOT must also be taken on by a task.

## Round 1 establishes coverage; Round 2+ consumes the delta

Round 1 first independently reads first-party source and SOT, then verifies the whole plan; the plan's paraphrase of the source is only a claim to be proven.
Round 1 may use dimension fan-out and fresh eyes to improve coverage, but subagents only produce candidates; the main agent must
re-verify, deduplicate, and produce the one and only `review.md`.

Round 2+ is a CLOSED WORLD:

- reconcile unclosed prior-round problems;
- review this round's diff;
- check contract contradictions or regressions directly introduced by the diff.

Incremental rounds do not explore fresh territory, do not rescan unchanged text, and do not promote execution-time state-machine details into plan problems.
Convergence comes from monotonically shrinking scope, not from a hard round cap or from continually manufacturing findings.

## The triage ledger is the memory of author dispositions

Every `TRIAGE_LEDGER` must be read and applied in time order from oldest to newest. A newer disposition on the same root cause overrides an older one.

- `rejected`: do not reopen it under a new skin; challenge it only when new evidence disproves its factual premise.
- `applied`: verify whether it is really fixed; only an incorrect repair produces a carried finding.
- `flagged`: the developer is deciding; do not copy it into another finding.

This keeps each lane's viewpoint independent without repeatedly asking the author to handle the same already-adjudicated matter.

## Adversarial posture raises the proof pressure

`--devils-advocate` means actively seeking counterexamples and testing assumptions the plan leaves unstated, while still bound by the same goal, scope, evidence standard,
and readiness rules. Suspicion is not default rejection; a counterargument also needs first-party evidence. Switching posture does not create a new
lane, nor does it turn an incremental round back into a full round.

## A finding is evidence, not a sales pitch

A finding states only location, observation, evidence, impact, and optional remediation information; it does not write "blocking", "recommend adopting", P0/P1, or
the source model. Only macro problems that would change the execution result enter 「新问题与建议」; wording drift that does not change delivery
enters 「同步清单」. The "other" dimension only catches qualified macro problems that fit no existing category, and must not bypass the stop rule;
implementation details, style nits, and execution-time control flow that is already clear enough are discarded outright.

The complete prior-round reconciliation table stays in `review.md` as an audit record; chat shows only the one-line summary generated by the renderer.
`本轮探索区域` likewise serves only later reviewers and does not enter chat. Every other section is preserved verbatim.

## The renderer enforces the output protocol

The agent's responsibility ends at writing a valid, complete `review.md`. The chat body is validated and deterministically generated by
`cli_extensions/review_artifact.py render-response`:

- validate the title, fixed sections, order, ledger, and verdict;
- remove the file-only 「本轮探索区域」;
- condense the complete prior-round reconciliation table into the existing summary;
- preserve all other content verbatim.

The skill therefore no longer asks the model to apply the same output discipline by hand. The final reply simply returns the renderer's product verbatim.
