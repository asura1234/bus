---
name: execute-plan
description: Execute a reviewed Bus plan task graph through deterministic owner, evidence, task-review, final-gate, and landing contracts. Use when asked to implement or execute a plan.
---

# Execute Plan

The main agent is the only orchestrator. Task agents implement only their assigned owner. No actor performs Git mutations until the automated final gate is complete; the workflow then invokes `commit-and-push` exactly once.

Read these files completely before starting:

- [Plan Execution Guide](../../docs/guides/plan-execution-guide.md)
- [Task Agent Report Format](../../docs/guides/task-agent-report-format.md)
- [Execute Plan Action Format](../../docs/guides/execute-plan-action-format.md)

This skill consumes plans that follow [the canonical plan template](../../docs/templates/plan-template.md), regardless of whether `create-plan` or a developer authored them.

## 1. Start or resume

`review-plan-complete` is the only first-run readiness state. A resumed execution accepts `plan-execution-in-progress`. Do not create a second readiness system from per-file checkboxes or completeness percentages.

```sh
python3 skills/execute-plan/scripts/execute_plan.py start --plan <CONTROL_PLAN>
```

Before changing plan state, the driver creates an immutable execution baseline, validates the plan and task graph, creates the active attempt manifest, and emits the first action envelope. Resume reuses the existing baseline and state.

## 2. Execute only the current action

Every action is bound to `ACTION_ID`, `EXPECTED_STATE_HASH`, `TASK_ID`, and `GENERATION`. A stale generation, reviewer, or pre-resume action cannot advance current state.

For `DISPATCH_TASK` and `REMEDIATE_TASK`:

1. Give the task agent every `INPUT_ARTIFACTS` path, the task identity, and its generation-specific `report.md` and `completion.txt` paths.
2. The task agent reads the task snapshot, this skill's guide, the report format, every explicit reference implementation, applicable `AGENTS.md`, and directly relevant golden tests before writing.
3. It writes only within its owner and generation report directory, runs its declared task gate, then obtains the exact scoped format command:

   ```sh
   python3 skills/execute-plan/scripts/task_scoped_lint.py \
     --repo <root> [--file <generation-delta-file>]...
   ```

   `NOT_APPLICABLE` means no currently existing Rust source needs scoped formatting. Otherwise execute the emitted command exactly and include its evidence in the task report.
4. It fingerprints the report with `task_agent_report.py`, renders `completion.txt`, returns the renderer output verbatim, and ends the turn.

`completion.txt` is the canonical completion signal. Mailbox events only wake the orchestrator early. When no independent work remains, wait on current-generation completion paths for at most 60 seconds with `task_completion_wait.py`; never wait on an old generation or busy-poll.

For `REVIEW_TASK`, run the emitted command exactly. The reviewer reads [Task Acceptance Guide](../../docs/guides/task-review-guide.md) and [Review Artifact Format](../../docs/guides/review-format.md), then writes the only `review.md` to `EXPECTED_OUTPUT`. One reviewer owns a round; reuse that reviewer for follow-up rounds when possible.

## 3. Consume artifacts

After a completion artifact appears:

```sh
python3 skills/execute-plan/scripts/execute_plan.py ingest-completion \
  --plan <CONTROL_PLAN> --action-id <ACTION_ID> \
  --expected-state-hash <EXPECTED_STATE_HASH>
```

The driver validates generation, snapshot, hashes, logs, test evidence, scoped formatting, actual files, and owner containment before emitting `REVIEW_TASK`.

After a review artifact appears:

```sh
python3 skills/execute-plan/scripts/execute_plan.py ingest-review \
  --plan <CONTROL_PLAN> --action-id <ACTION_ID> \
  --expected-state-hash <EXPECTED_STATE_HASH>
```

Follow only the new action:

- `REMEDIATE_TASK`: the original task agent reads `review.md`, repairs accepted findings within its owner, reruns affected task gates and generation-scoped formatting, and publishes a fresh generation report.
- `PLAN_REPAIR_REQUIRED`: freeze dispatch and resolve the structured `upstream-contract`, `owner-graph-contract`, `needs-context`, or `developer-decision` reason.
- `DISPATCH_TASK` or `REVIEW_TASK`: continue normally.
- `FINALIZE`: enter the final-tree phase.

After a structured repair:

```sh
python3 skills/execute-plan/scripts/execute_plan.py resume-repair \
  --plan <CONTROL_PLAN> [--context <artifact>]
```

The driver revalidates the graph and creates a fresh attempt/generation. Never hand-edit state. Each task attempt has at most three review rounds; Round 2 and Round 3 are closed world. If the third round remains blocked, stop for orchestrator diagnosis.

## 4. Final tree and landing

Once every task is Ready, run only the final gates declared by the plan, in canonical Bus order:

```text
just lint -> just test -> optional just build
```

Bind each gate to the current tree through the driver:

```sh
python3 skills/execute-plan/scripts/execute_plan.py run-final \
  --plan <CONTROL_PLAN> --kind <lint|unit|build> -- <canonical command>
```

Main runs full-tree gates, not each task gate again. Route failures to the original file owner; after any repair changes the tree, restart at final lint. A failed final gate returns the plan to `plan-execution-in-progress`.

When declared final gates pass, freeze the exact worktree tree:

```sh
python3 skills/execute-plan/scripts/execute_plan.py finalize \
  --plan <CONTROL_PLAN> [--require-build] [--manual-e2e]
```

Only `STATE=READY_TO_COMMIT` permits the sole Git mutation: invoke `commit-and-push`. Do not format, generate, or edit afterward. Then run:

```sh
python3 skills/execute-plan/scripts/execute_plan.py verify-landing \
  --plan <CONTROL_PLAN>
```

The proof must show that all plan delta is committed, the pre-existing dirty baseline is unchanged and uncommitted, `HEAD^{tree}` equals the verified tree, and `origin/<current-branch>` equals local HEAD. Bus permits direct `master` landing only when the user explicitly authorized it through `commit-and-push`; never force-push it.

## 5. Stop boundary

Only `STATE=COMPLETE` is completion. Return:

```text
STATUS=plan-execution-complete
HEAD=<committed-and-pushed-head>
TREE=<HEAD^{tree}>
AUTOMATED_EVIDENCE=<artifact paths>
MANUAL_VERIFICATION=<required | not-required>
NEXT=developer manual verification, then review-pr
```

Stop. Do not automatically rebase or create a PR.
