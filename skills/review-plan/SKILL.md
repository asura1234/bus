---
name: review-plan
description: Iteratively and read-only review a plan created from the project template. Round 1 verifies architecture, completeness, validation, and task-graph safety; Round 2+ reconciles the prior round and reviews only the current delta and its direct consequences. Supports stable reviewer lanes, author triage ledgers, adversarial posture, narrow subagent fan-out, and deterministic review rendering. Use when asked to review or re-review a plan under plans/.
---

Read [guide.md](./guide.md) for workflow principles. The source of truth for macro findings, task-graph safety, admissibility, and verdicts is [plan-review-guide.md](../../docs/guides/plan-review-guide.md). This file defines only executable control flow. Input must follow the canonical project plan template; it does not require a private create-plan session.

Plans written before this migration remain historical inputs but are not
silently coerced into the new machine contract. The prologue identifies the
legacy status form and stops with one migration instruction: explicitly invoke
`create-plan` to build a canonical successor from
`docs/templates/plan-template.md`, preserving the developer-owned goal,
non-goals, and decisions. `review-plan` stays read-only and never rewrites the
historical file itself.

```text
INPUT $ARGUMENTS =
  [<plan-file>] [--reviewer <lane>] [--devils-advocate]

DEFAULT reviewer = default, only when the prologue can select a lane unambiguously

========== 1. READ-ONLY BOUNDARY ==========

MUST NOT:
  - modify the plan or repository files
  - check file checkboxes or rewrite plan state
  - git add / commit / checkout / stash / rebase

MAY:
  - write this skill's round artifacts under temp/review-plan/
  - run read-only queries, focused tests, and builds needed to verify plan claims

========== 2. DETERMINISTIC PROLOGUE ==========

Run with an explicit Bus plan path:
  python3 skills/review-plan/scripts/review_round.py \
    <plan-file> [--reviewer <lane>] [--devils-advocate]

IF exit != 0 AND output says a bare invocation is ambiguous because multiple or named lanes exist:
  IF the developer supplied a lane, or said to reuse the previous lane and this conversation has a successful prior REVIEWER:
    the value must exactly match one of the lanes listed by this failed prologue;
    confirm approximate names with the developer;
    rerun immediately with that exact --reviewer value.
  ELSE:
    reproduce stdout/stderr verbatim, request the exact lane, and STOP.
ELSE IF exit != 0:
  reproduce stdout/stderr verbatim and STOP.

IF NOTE says plan state is already in review but this lane has no history:
  tell the developer the lane restarts with Round 1 full rules.

Capture:
  ROUND, MODE, POSTURE, REVIEWER, BRANCH, PLAN, STATE_DIR,
  SNAPSHOT, DIFF, PREV_REVIEWS, TRIAGE_LEDGER

Read(docs/guides/orchestrated-room-brief.md) completely, then run:
  python3 cli_extensions/room_assignment_context.py [--frame "<frame>"] \
    --output <STATE_DIR>/room-assignment-context.json
IF exit != 0:
  reproduce stdout verbatim as blocker evidence and STOP.
ORIGIN = ASSIGNMENT_ORIGIN

========== 3. LOAD LOCKED CONTEXT ==========

Read(skills/review-plan/guide.md) completely
Read(docs/guides/plan-review-guide.md) completely
Read(docs/guides/consumer-fallout-format.md) completely
plan = Read(PLAN) completely

IF ORIGIN == verified:
  the plan's goal and non-goals must equal the context file's goal and non_goals exactly;
  on mismatch, report both texts as blocker evidence for the Orchestrator and STOP without writing review.md.
IF ORIGIN == NotInBusRoom:
  the plan's own goal and non-goals are the locked input.

The goal, non-goals, archived decisions, and one-shot workflow are locked inputs. Do not rewrite, expand, or overturn them. Plan content that violates them may be a finding. Only new evidence that disproves the factual premise of an archived decision may produce a one-line decision-premise challenge for the developer.

IF TRIAGE_LEDGER != none:
  read every comma-separated ledger from oldest to newest;
  for the same location and semantic root cause, the newest disposition wins:
    rejected -> do not re-raise unless new evidence disproves its factual premise
    applied  -> verify only the repair; carry forward only an incorrect repair or new problem
    flagged  -> do not duplicate as a finding

========== 4. POSTURE ==========

IF POSTURE == devils-advocate:
  actively falsify claims as defined by guide.md without changing scope, lane, MODE, or evidence standards.
ELSE:
  use standard posture.

========== 5. MODE = full ==========

IF MODE == full:
  read the plan's references, relevant source, applicable AGENTS.md files, and repository architecture sources of truth before checking the plan. A plan's paraphrase is never evidence.

  fallout = <dirname(SNAPSHOT)>/consumer-fallout.json
  Run:
    python3 skills/review-plan/scripts/consumer_fallout.py \
      --plan <PLAN> --output <fallout>

  Consume only stdout's bounded, per-task, high-confidence unresolved summary. Do not load the complete artifact wholesale. When omitted_unresolved_count > 0, investigate only the relevant task bucket. A candidate closes only when file contract, owner, and gate coverage are all established or the exclusion is explicit.

  IF runtime supports subagent dispatch:
    invoking this skill authorizes read-only narrow fan-out for this review round.
    Use the fewest groups justified by plan size:
      architecture = multiple purposes, boundaries, dependency direction, duplicate mechanisms, implicit decisions
      completeness = missing files/tasks/tests, internal contradictions, mismatches with source facts
      task-graph safety = dependencies, isolation, producer/consumer contracts, hidden semantic cycles, false edges, false independent acceptance
      evidence = ambiguity, test scenarios, whether validation commands prove their claims, and other admitted macro concerns

    Every subagent must:
      - Read(docs/guides/plan-review-guide.md)
      - Read(PLAN) completely plus first-party source/SOT needed for its group
      - review only its assigned dimensions and remain read-only
      - honor locked goals, non-goals, archived decisions, and one-shot execution
      - produce raw candidates only:
        plan location / observation / evidence / impact / optional remediation

    One checklist-free fresh-eyes subagent is optional.
    No source writes review.md independently.
    The main agent returns to first-party evidence, verifies and deduplicates by location and semantic root cause, removes nits and triaged items, and closes every subagent before reporting.
  ELSE:
    the main agent covers every dimension serially.

  GOTO REPORT

========== 6. MODE = incremental ==========

IF MODE == incremental:
  prev_reviews = Read every file in PREV_REVIEWS
  diff = Read(DIFF)

  FOR EACH prior-round finding not yet closed:
    reread the current plan, diff, and necessary first-party evidence;
    record exactly one reconciliation state under `前轮问题核销`:
      satisfactory | rejected | withdrawn |
      partially-addressed | not-addressed | disputed
    for partially-addressed / not-addressed / disputed:
      restate the same root cause under `新问题与建议`, with `(承 R<round>-<number>)` in the title.

  Review every delta hunk with Round 1 rigor.

  CLOSED WORLD:
    a new finding may originate only from:
      1. an unresolved prior finding
      2. the current diff
      3. a contract contradiction or regression directly caused by that diff
    do not explore previously unread source, uncovered scenarios, or untracked tasks;
    do not raise a new finding against unchanged text;
    do not rerun full fresh-eyes or full-dimension fan-out.

  IF runtime supports subagent dispatch AND narrow help is useful:
    dispatch only against prior findings, changed task-graph hunks, direct callers, or direct contract dependencies of changed hunks;
    collect, deduplicate, and close the subagents.

========== 7. REPORT ==========

REPORT:
  Every finding is an evidence-backed fact:
    location / observation / evidence / impact / optional remediation
  Do not attach severity, blocking, adoption recommendations, or source-agent labels.
  Put substantive problems in `新问题与建议` and wording-only consistency drift in `同步清单`.
  When a section is empty, write only `无。`.

  Verdict:
    Ready:
      `新问题与建议` is empty and every prior finding is closed at root cause
    Needs Refinement:
      a locally repairable substantive problem exists; never escalate because of round count
    Abandon:
      the plan has multiple purposes, a fundamentally wrong architecture, or cannot be repaired locally

  Before completion, Read(docs/guides/review-format.md) completely.
  Write <round-NN/review.md> using the plan-review template exactly.
  Recommend state only; the reviewer does not mutate plan state.

========== 8. DETERMINISTIC RESPONSE GATE ==========

Run:
  python3 cli_extensions/review_artifact.py render-response \
    --review-file <round-NN/review.md> \
    --output <round-NN/chat-response.md>

IF exit != 0:
  repair review.md and rerun; never report a verdict first.

Read(chat-response.md) completely.
The final response MUST equal chat-response.md verbatim.
Do not summarize, rewrite, add an introduction, add links, add a conclusion, or alter the verdict.

STOP. The developer decides whether to adopt findings, reconcile multiple lanes, or update plan state.
```
