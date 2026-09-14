# Plan Execution Guide

This is the shared principle source of truth for task agents and the main orchestrator. A reviewed plan defines each task's goal, owner, dependencies, tools, constraints, and acceptance gate; the implementing agent chooses the method within those boundaries.

An owner is the task's exclusive write and scheduling boundary, not an exact expected diff. Prefer stable module or subsystem directories so a task can add a directly required helper or test inside its owner and record that deviation. Ownership never authorizes unrelated changes.

## Single-task contract

The task agent reads the task-stripped snapshot, this guide, and `task-agent-report-format.md` completely. The snapshot is canonical and contains the plan title, goal, non-goals, archived decisions, complete file contract, and only this task block.

- Write only within `owned files` and the assigned generation's `report.md`, `completion.txt`, and gate logs. Never edit another owner, execution state, task checkbox, or plan status.
- If a required path falls outside the owner, report `STATUS=OWNER_GAP` before touching it. Fill the report's owner-gap section, render completion, return it verbatim, and stop.
- Depend only on the execution baseline, delivered snapshot, and declared upstream artifacts.
- Choose implementation details independently while satisfying the task constraints and locked goal.
- Run the task acceptance gate, then the runtime-generated scoped Rust format command when applicable. Record commands, workdir, exit status, non-zero test count, relevant coverage evidence, and raw log hashes. Exit zero with zero selected tests or the wrong harness is not valid evidence.

## Task reports and waiting

Full evidence lives in the report artifact. Cross-agent completion is only the short deterministic envelope. Never hand-summarize renderer output or replace a closed status with free text. A malformed report is incomplete until corrected and re-rendered.

Every dispatch or remediation gets a monotonically increasing generation directory. Old generations remain audit evidence but leave the live wait set. `DONE` means evidence was published, not that the task was accepted. `DONE_WITH_CONCERNS`, `OWNER_GAP`, `NEEDS_CONTEXT`, and `BLOCKED` require immediate orchestrator handling.

Do not busy-poll. Work on other ready orchestration first, then wait on any current-generation completion for a bounded interval. A timeout is not failure. Validate a returned report immediately and begin its task review while independent agents continue.

## Scheduling and performance

The driver stores identities and accepted artifact references, not duplicate report or review bodies. Artifacts are the evidence source of truth; state answers only what action is currently legal.

Tasks run narrow `[TASK_LOCAL]` gates and scoped formatting once. The main agent runs full-tree final gates only after every task is Ready. It does not repeat each task gate. Manual checks bind to the committed tree and never masquerade as automated evidence.

Each task attempt has at most three acceptance rounds. Later rounds reconcile existing findings and their direct consequences only. Performance policy is intentionally small: avoid duplicate gates, keep status output short, and report one truthful update when state has not advanced for a meaningful interval.

## Task graph and shared-tree safety

- `blocked-by` is the sole scheduling dependency. Invalid graphs fail closed.
- A task becomes active only when dependencies are Ready, inputs exist, and owners are disjoint.
- Before an active attempt, create a manifest and prove `attempt delta` is contained by the union of active owners. Capture tracked, untracked, deleted, renamed, and symlink state with content hashes.
- On completion, capture another checkpoint, verify active-owner containment, then derive that task's actual files.
- After task review returns Ready, update its checkbox, create a fresh attempt, and unlock downstream work without a whole-wave barrier.
- Stop on out-of-owner writes, omitted report paths, two tasks claiming one path, or unexplained new dirty files.

### Repair

Task review returns only `Ready`, `Needs Refinement`, or `Plan Repair Required`. Structured repair reasons are:

- `upstream-contract`: reopen the named producer and every direct consumer;
- `owner-graph-contract`: return owner, dependency, or task-contract repair to the plan author;
- `developer-decision`: stop for developer adjudication;
- `needs-context`: provide missing evidence through a fresh attempt.

Repair always creates a new attempt and generation. Never rewrite old manifests or absorb unaccepted delta into the baseline.

## Quality requirements

### Minimal change

- Modify only what the task requires; do not reorder, rewrite, or format unrelated code.
- Preserve repository naming, style, patterns, and architecture.
- Plan snippets lock public shape and control flow, not the literal implementation.

### Correctness and tests

- Preserve error handling, logging, concurrency, resource-lifetime, platform, and architecture contracts.
- Add or update only relevant tests. A task gate must run independently after its declared upstream tasks complete.
- Scoped formatting is required when generated, but it is not functional evidence. Full-tree lint belongs to finalization; never silence it with ignores.

### Repository source-of-truth updates

When a task changes a public contract or skill architecture, its owner and gate also cover the corresponding durable documentation. In Bus, `skills/AGENTS.md` and `skills/skill-architecture.md` describe skill architecture; public behavior belongs in the appropriate `README.md`, `docs/`, or release document. Do not invent per-module `AGENTS.md` files where the repository has no such convention.

## Task acceptance

For every task, the orchestrator performs mechanical owner verification, report/gate validation, task review, and owner-local remediation as needed. Only current-hash evidence, a Ready task review, and active-attempt isolation permit the checkbox to close and downstream work to unlock.

A substantive change after Ready invalidates old evidence. A sync-only exception is allowed only when triage proves the delta is exactly a listed consistency item and the affected narrow gate is rerun. Task review does not narrow final PR review.

## Zero Git during execution and the final tree

From the first execution gate until every automated final gate passes, no actor stages, commits, pushes, stashes, rebases, or switches refs. The sole exception is the final `commit-and-push` call after `STATE=READY_TO_COMMIT`; no tree-changing action follows it.

The control plan is the only external input. The first run creates an immutable baseline before changing plan status. Resume validates and reuses it. Pre-existing dirty paths cannot overlap task owners, change during execution, or enter plan delta.

The final sequence is `just lint`, `just test`, then optional `just build`. Record each command, tree, exit code, duration, and log hash. Route failures to their owners and restart from lint after a repair changes the tree. The final tree includes task checkboxes, deviations, and `plan-execution-complete`. Manual testing records the committed tree and checklist for the developer; final `review-pr` remains independent.
