---
name: review-plan
description: Iteratively and read-only review a plan file that follows the project plan template. Round 1 verifies architecture, completeness, testing, and task-graph safety in full; Round 2+ only reconciles the prior round and reviews the current delta and its direct consequences. Supports stable reviewer lanes, author triage ledgers, adversarial posture, and narrow subagent fan-out; the final review.md is deterministically rendered by a script into the chat reply. Use when the user asks to review or re-review a plan under plans/.
---

Review principles are in [guide.md](./guide.md); the SOT for macro judgment, task-graph safety, admissibility, and verdicts is
[plan-review-guide.md](../../docs/guides/plan-review-guide.md). This file defines only the executable flow.
Input only needs to follow `docs/templates/plan-template.md`; it does not require the session that wrote the plan or any other private state.

Plans written before this migration remain historical inputs but are not
silently coerced into the new machine contract. The prologue identifies the
legacy status form and stops with one migration instruction: write a canonical
successor from `docs/templates/plan-template.md`, preserving the developer-owned goal,
non-goals, and decisions. `review-plan` stays read-only and never rewrites the
historical file itself.

```text
INPUT $ARGUMENTS =
  [<plan-file>] [--reviewer <lane>] [--devils-advocate]

DEFAULT reviewer = default (only when the prologue can select a lane unambiguously)

========== 1. READ-ONLY BOUNDARY ==========

MUST NOT:
  - modify the plan or repository files
  - check file checkboxes or rewrite plan state
  - git add / commit / checkout / stash / rebase

MAY:
  - write this skill's round artifacts under temp/review-plan/
  - run the read-only queries, tests, and builds needed to verify plan claims

========== 2. DETERMINISTIC PROLOGUE ==========

Run:
  python3 skills/review-plan/scripts/review_round.py \
    [<plan-file>] [--reviewer <lane>] [--devils-advocate]

IF exit != 0 AND output says multiple / named lanes make a bare invocation ambiguous:
  IF the developer already gave a lane, or said "use the previous lane" and this conversation has the REVIEWER of the last successful prologue:
    the value must exactly equal an existing lane listed by this FAIL; an approximate name must be confirmed with the developer first.
    Add `--reviewer <lane>` with that exact lane value and immediately rerun the prologue.
    Do not reply with only the lane name, and do not make another bare invocation.
  ELSE:
    reproduce stdout/stderr verbatim, ask the developer for the exact lane, then STOP.
ELSE IF exit != 0:
  reproduce stdout/stderr verbatim, then STOP.

IF NOTE says the state has entered review but this lane's history is lost:
  tell the developer this lane restarts under Round 1 full rules.

Capture:
  ROUND, MODE, POSTURE, REVIEWER, BRANCH, PLAN, STATE_DIR,
  SNAPSHOT, DIFF, PREV_REVIEWS, TRIAGE_LEDGER

========== 3. LOAD LOCKED CONTEXT ==========

Read(skills/review-plan/guide.md) completely
Read(docs/guides/plan-review-guide.md) completely
Read(docs/guides/architecture-principles.md) completely
Read(docs/guides/consumer-fallout-format.md) completely
plan = Read(PLAN) completely

The goal, non-goals, archived decisions, and one-shot workflow are locked inputs; do not rewrite, expand, or overturn them.
Plan content that violates the locked inputs may become a finding. Only when new evidence disproves the factual premise of a decision
may you output a one-line "decision-premise challenge" for the developer to adjudicate.

IF TRIAGE_LEDGER != none:
  decided = Read every comma-separated TRIAGE_LEDGER, oldest to newest
  for the same location + semantic root cause, the newest disposition wins:
    rejected -> do not re-raise, unless new evidence disproves its factual premise
    applied  -> verify only the repair; carry forward only an incorrect repair or a newly introduced problem
    flagged  -> do not duplicate as a finding

========== 4. POSTURE ==========

IF POSTURE == devils-advocate:
  actively falsify as defined by guide.md, without changing scope, lane, MODE, or the evidence standard.
ELSE:
  use the standard posture.

========== 5. MODE = full ==========

IF MODE == full:
  first read the material the plan references and the relevant source,
  independently establish the current state, then verify the plan's claims; the plan's paraphrase itself is never evidence.
  fallout = `<dirname(SNAPSHOT)>/consumer-fallout.json`
  Run `python3 skills/review-plan/scripts/consumer_fallout.py --plan <PLAN> --output <fallout>`.
  Consume only stdout's per-task, high-confidence unresolved summary; do not load the complete artifact into context wholesale.
  When `omitted_unresolved_count > 0`, gather evidence only in the relevant task bucket; a candidate must be verified as closed on file contract, owner,
  and gate, or explicitly excluded.

  IF runtime supports subagent dispatch:
    the user invoking this skill explicitly authorizes read-only narrow fan-out for this round.
    Use the fewest groups necessary for the plan's size:
      architecture = multiple purposes, boundaries, dependency direction, duplicate mechanisms, implicit decisions
      completeness = missing files/tasks/tests, internal contradictions, mismatches with source facts
      task-graph safety narrow subagent =
        dependencies, isolation, producer/consumer, hidden semantic cycles, false edges, false independent acceptance
      evidence = ambiguity, test scenarios, whether validation commands truly cover the goal, other macro problems admitted under the strict bar

    Every **dimension-group** subagent must (except the fresh-eyes subagent, see below):
      - Read(docs/guides/plan-review-guide.md) and docs/guides/architecture-principles.md
      - Read(PLAN) completely plus the first-party source/SOT its group needs
      - review only its assigned dimensions, read-only, never modifying the repo
      - honor the locked goals, non-goals, archived decisions, and one-shot workflow
      - make macro judgments only and return raw candidates:
        plan location / observation / evidence / impact / optional remediation

    **A fresh-eyes subagent must also be dispatched.** It is this round's source of viewpoint diversity, not an optional supplement:
    the groups above all start from the same checklist and find what the checklist expects; problems the checklist cannot name
    are only stumbled upon by an agent that never read the checklist.
    The ground truth it receives must be **verbatim identical** to the other groups' — the same PLAN snapshot, the same goal /
    non-goals / archived decisions; the only allowed difference is "no checklist".
    It **must not** read plan-review-guide.md, architecture-principles.md, the dimension lists, or any prior review: an agent that has read the checklist
    is no longer fresh eyes; it will confirm the checklist item by item instead of reading the plan.
    §1 READ-ONLY BOUNDARY and the locked inputs apply to it in full, not relaxed by a single word.
    Every source produces candidates only; none writes its own review.md.
    The main agent returns to first-party evidence, verifies each candidate, deduplicates by location + semantic root cause,
    discards nitpicks and items already decided in triage, then closes every subagent of this round.
  ELSE:
    the main agent covers every dimension serially.

  GOTO REPORT

========== 6. MODE = incremental ==========

IF MODE == incremental:
  prev_reviews = Read every file in PREV_REVIEWS
  diff = Read(DIFF)

  FOR EACH prior-round finding not yet closed:
    reread the current plan, the diff, and necessary first-party evidence.
    In 「前轮问题核销」 record one and only one state:
      satisfactory | rejected | withdrawn |
      partially-addressed | not-addressed | disputed
    partially-addressed / not-addressed / disputed:
      restate the same root cause in 「新问题与建议」, with 「承 R<round>-<number>」 in the title.

  Review every delta hunk with Round 1 rigor.

  CLOSED WORLD:
    only the following sources may enter this round as new findings:
      1. an unclosed prior-round finding
      2. something directly introduced by this round's diff
      3. a contract contradiction or regression directly caused by the diff's changes
    do not explore previously unread source, uncovered scenarios, or untracked tasks;
    do not raise a brand-new finding against unchanged text;
    do not rerun full fresh-eyes or the full dimension fan-out.

  IF runtime supports subagent dispatch AND narrow help is useful:
    dispatch only against prior-round findings, this round's task-graph changed hunks,
    and the direct callers or contract dependencies of changed hunks.
    Collect, deduplicate, and close the subagents.

========== 7. REPORT ==========

REPORT:
  Every finding must be an evidence-backed fact:
    location / observation / evidence / impact / optional possible remediation
  Do not attach severity, blocking, adoption recommendations, or source-agent labels to a finding.
  Substantive problems go in 「新问题与建议」; wording-only consistency drift goes in 「同步清单」.
  When a section has no content, write only 「无。」.

  Verdict:
    可执行（Ready）:
      「新问题与建议」 is empty, and every prior-round problem is closed at root cause
    需要完善（Needs Refinement）:
      a locally repairable substantive problem exists; never escalate because of round count
    废弃（Abandon）:
      multiple purposes, a fundamentally wrong architecture, or cannot be salvaged locally

  Before completion, Read(docs/guides/review-format.md) completely.
  Write <round-NN/review.md> strictly using the plan-review template.
  Recommend state only; the reviewer does not write plan state.

========== 8. DETERMINISTIC RESPONSE GATE ==========

Run:
  python3 cli_extensions/review_artifact.py render-response \
    --review-file <round-NN/review.md> \
    --output <round-NN/chat-response.md>

IF exit != 0:
  repair review.md and rerun; never report the verdict first.

Read(chat-response.md) completely.
The final response MUST equal chat-response.md verbatim.
Do not hand-summarize, rewrite, add an introduction, add links, add a conclusion, or change the verdict.

STOP. Whether to adopt findings, reconcile multiple lanes, and write plan state are all decided by the developer.
```
