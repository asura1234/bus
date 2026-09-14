# Task Acceptance Guide

This is the principle source of truth for task review inside `execute-plan`. Task acceptance occurs after one task implementation and decides only whether that task can close independently under the reviewed plan. It does not replace final `review-pr`.

## Acceptance objective

Verify only:

- **Completion:** task goal, constraints, and expected artifacts are implemented without omitting a required result.
- **Plan adherence:** implementation matches the task block and locked goal, non-goals, and archived decisions; any deviation is recorded and does not change the assignment.
- **File isolation:** every touched file is within this task's owner boundary.
- **Direct contracts:** declared `produces` exist; `consumes` is checked only against direct upstream artifacts, without reviewing upstream internals or incomplete downstream work.
- **Gate evidence:** task report binds the current generation, snapshot, and content hashes; command, workspace, exit code, non-zero test count, coverage, logs, and hashes are genuine; runtime-provided scoped lint binds exactly to existing generation-delta files. The reviewer does not rerun lint or invent an additional lint prerequisite.

Task acceptance is not full code review. It does not perform repository-wide fresh-eyes exploration or judge unrelated refactors, hardening, performance work, or style. Final code still enters the complete `review-pr` loop.

## Evidence boundary

Read the mechanical prologue's task-contract snapshot, every actual touched file, validated task report and gate logs, prior review in the same task lane, and task-mode triage. Read direct upstream artifacts or callers only to validate `produces`/`consumes`. Do not review other task implementations or read other task lanes.

`FILES` is the required per-file read set. The artifact records only file count, `scope-inputs.json#files`, and `SCOPE_HASH`, not another long duplicated path list. Artifact rounds continue across repair attempts; driver-local attempt review rounds reset on a fresh attempt. They are different counters.

Do not trust an implementing agent's prose alone for:

- actual files versus owner containment;
- goal, constraint, artifact, and implementation correspondence;
- gate commands, exits, and baseline comparisons;
- whether a deviation changes task boundary or downstream contract.

Mechanical identity has two levels. `PLAN_CONTEXT_HASH` covers title, goal, non-goals, and complete archived decisions. `TASK_EVIDENCE_HASH` (compatibility alias `SCOPE_HASH`) additionally covers this task's intersecting file contract, selected task block, direct consumed producer blocks, actual file contents, and task-report hash. The snapshot retains the full file contract for anti-entropy reading, but unrelated sibling corrections do not invalidate this task's evidence.

The reviewer does not run tests by default. Run one narrow reproduction only when report evidence is incomplete or suspicious, or when a finding cannot otherwise be established. Never make task review a third full gate run.

## Round convergence

- Round 1 completely reads the selected task contract, actual files, and evidence, and reports every local issue found in that pass.
- Round 2 is closed world: reconcile Round 1 findings and inspect their direct repair consequences only.
- Round 3 reconciles remaining Round 2 findings and direct consequences only. If a substantive issue remains, stop the ordinary loop and classify it as root cause not repaired, upstream contract breach, owner/graph gap, or developer decision.
- An upstream or task-graph repair creates a fresh attempt/generation. Artifact round numbering remains continuous, but fresh-eyes review does not restart.

Use one reviewer per round and reuse that reviewer when possible. Parallel dimension fan-out and fresh eyes belong to final PR review.

## File isolation and graph repair

An owner-local repair yields `Needs Refinement`. A repair that requires reopening upstream work, changing owner/dependency/task contracts, or developer adjudication yields `Plan Repair Required` with the structured reason from the format SOT. Do not create free-text orchestration states.

Read-only access to a direct dependency does not transfer write ownership. `blocked-by`, `produces`, and `consumes` never override the single-writer owner contract.

## Findings and verdicts

Use the task section of [Review Artifact Format](review-format.md). SUBSTANTIVE problems enter `新问题与建议`; wording, naming, or documentation drift that cannot change delivery enters `同步清单（CONSISTENCY drift，非阻塞）`. Deduplicate by root cause. Every SUBSTANTIVE finding cites the exact goal, constraint, owner, produces, consumes, or gate clause it prevents from closing. General quality opinions without a local task-clause relationship belong to final PR review and cannot block task Ready.

The three verdicts are fixed:

- `Ready`: goal, constraints, owner, direct contracts, and gate evidence all hold; there is no current SUBSTANTIVE finding and prior findings are closed at root cause.
- `Needs Refinement`: an owner-local substantive issue is repairable.
- `Plan Repair Required`: structured recovery is needed. `upstream-contract` names the direct producer; `owner-graph-contract` returns to the plan author; `developer-decision` waits for the developer.

Consistency drift does not block Ready or trigger another round.

## Read-only and artifact boundary

The reviewer cannot modify plan, implementation, tests, checkboxes, or gate evidence and cannot stage, commit, or push. The only allowed write is the current task lane `review.md` at the explicit output path. The round script owns snapshots and bookkeeping.
