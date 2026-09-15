---
name: review-pr
description: Iterative code review in which behavioral suspicions in correctness, security, and bug dimensions are proven by a failing test when possible; write access is limited to test files and all other dimensions are read-only. Round 1 reviews the committed non-plan diff against an explicit base; Round 2+ reconciles prior findings and reviews only the new delta and its direct consequences. Supports stable reviewer lanes, an optional plan, author triage ledgers, adversarial posture, narrow subagent fan-out, and deterministic review rendering. Use for PR, branch, or follow-up code review.
---

Read [guide.md](./guide.md) for workflow principles. The nine dimensions, severity semantics, and guardrails are defined by [code-review-guide.md](../../docs/guides/code-review-guide.md). This file defines only executable control flow.

```text
INPUT $ARGUMENTS =
  [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan-file>]

DEFAULT base = origin/master
DEFAULT reviewer = default, only when the prologue can select a lane unambiguously

========== 1. VERIFICATION BOUNDARY ==========

SCOPE:
  - apply the code-review guide's goal-relevance gate before every investigation, proof attempt, delegation, or output; the current round boundary can narrow that scope further
  - dimension 6 is the only exception: classify out-of-goal diff slices by purpose and cumulative size, but do not review their implementation
  - test proof applies only to runtime-behavior claims in dimensions 2 correctness, 3 security, and 7 bugs
  - other dimensions are established by reading source and cite only path:line plus the source observation; they do not create probes or use proven/unproven labels

SHOULD for dimensions 2 / 3 / 7:
  - turn every behavioral suspicion into a test before deciding whether it is a finding; create a test file or add a new case to an existing test file
  - run only the narrowest relevant Cargo test target or exact test name
  - a probe must have a meaningful failure mode and must turn red when the implementation is wrong; assertions about the harness itself or a mock configured by the test prove nothing

MAY:
  - write this skill's round artifacts under temp/review-pr/; review locks come only from pr_goal_context.py
  - run read-only source and Git queries

MUST NOT:
  - modify, create, or delete any non-test file
  - alter or roll back implementation to manufacture a red test
  - git add / commit / checkout / stash / rebase
  - run formatters, lint fixers, or other write commands
  - run quality gates such as full lint, format, coverage, build, or E2E; those belong to CI and gate-and-fix
  - run the complete unfiltered test suite

PROBE TEST LIFECYCLE:
  - probe tests remain uncommitted in the worktree for the author to adopt, rewrite, or remove
  - follow the guide's probe naming and ownership rules; assign non-overlapping test files before parallel work; never overwrite another probe or weaken an existing assertion
  - red -> create a finding in the matching dimension; the first evidence item is
    `已证明：<test @ relative test path> — <failed assertion>`
  - green -> withdraw the suspicion; keep the probe in the worktree and record one line under `本轮探索区域 / 运行的测试`
  - never leave a red probe without the corresponding finding

UNPROVEN SUSPICION:
  - behavior that Unit or Integration tests can prove must be proven before reporting
  - behavior confirmable only in a real terminal session may remain a suspicion; the first evidence item is exactly `未证明`, with no explanation or attempted-method log
  - phrase the observation as “cannot rule out X” and the impact conditionally
  - `未证明` does not block Ready, trigger a new round, or justify changing manually verified behavior by itself

========== 2. DETERMINISTIC PROLOGUE ==========

Read(docs/guides/orchestrated-room-brief.md) completely, then run:
  python3 cli_extensions/room_assignment_context.py [--frame "<frame>"] \
    --output temp/review-pr/room-assignment-context.json
IF exit != 0:
  reproduce stdout verbatim as blocker evidence and STOP.
ORIGIN = ASSIGNMENT_ORIGIN
IF ORIGIN == verified:
  Run:
    python3 skills/pr/scripts/pr_goal_context.py \
      --branch "<current branch>" --output temp/review-pr/goal-context.md \
      --assignment-context temp/review-pr/room-assignment-context.json [--plan <plan-file>]
  IF it fails: reproduce stderr verbatim as blocker evidence and STOP.

Run:
  python3 skills/review-pr/scripts/review_round.py \
    --base <ref> [--reviewer <lane>] [--devils-advocate] [--plan <plan-file>]

Always pass `--base origin/master` unless the developer supplied another explicit base.

IF exit != 0 AND output says a bare invocation is ambiguous because multiple or named lanes exist:
  IF the developer supplied a lane, or said to reuse the previous lane and this conversation has a successful prior REVIEWER:
    the value must exactly match one of the lanes listed by this failed prologue;
    confirm approximate names with the developer;
    rerun immediately with that exact --reviewer value.
  ELSE:
    reproduce stdout/stderr verbatim, request the exact lane, and STOP.
ELSE IF exit != 0 AND FAIL names missing, blank, or mismatched locked Goal/Non-goals:
  IF ORIGIN == verified:
    reproduce stdout verbatim as blocker evidence and STOP.
  IF ORIGIN == NotInBusRoom AND --plan was supplied:
    run pr_goal_context.py --branch <branch> --output temp/review-pr/goal-context.md --plan <plan-file>;
    rerun the prologue once; reproduce any remaining FAIL verbatim and STOP.
  IF ORIGIN == NotInBusRoom AND no plan was supplied:
    ask the developer for the PR's one-sentence Goal and its explicit Non-goals (`无` when none);
    save each reply verbatim to ignored temp goal and non-goal files, trimming only leading/trailing whitespace;
    do not infer either from the diff, commits, or PR description;
    run pr_goal_context.py --branch <branch> --output temp/review-pr/goal-context.md --goal-file <goal-file> --non-goal-file <non-goal-file>;
    rerun the prologue.
ELSE IF exit != 0:
  reproduce stdout/stderr verbatim and STOP.

IF NOTE says the worktree is dirty:
  tell the developer this round reviews committed changes only;
  probes created during this round also remain outside diff scope, finding counts, and verdict calculation.

Capture:
  ROUND, MODE, POSTURE, REVIEWER, BRANCH, BASE, HEAD, STATE_DIR,
  DIFF_SNAPSHOT, DIFF_DELTA, PREV_REVIEWS, PLAN, LOCKED_GOAL_FILE,
  LOCKED_NON_GOALS_FILE, TRIAGE_LEDGER

The prologue excludes plan documents from the committed code-review diff. Plan changes do not enter touched-file scope, findings, consistency drift, or the PR single-purpose calculation.

========== 3. LOAD LOCKED CONTEXT ==========

Read(skills/review-pr/guide.md) completely
Read(docs/guides/code-review-guide.md) completely
diff = Read(DIFF_SNAPSHOT)

IF PLAN != none:
  plan = Read(PLAN) completely
  locked_goal = the plan's goal verbatim, equal to LOCKED_GOAL_FILE
  non_goals = the plan's non-goals verbatim, equal to LOCKED_NON_GOALS_FILE
  archived_decisions = the plan's archived decisions verbatim, when present
ELSE:
  locked_goal = Read(LOCKED_GOAL_FILE)
  non_goals = Read(LOCKED_NON_GOALS_FILE)
  archived_decisions = none

An associated plan supplies only locked goal, non-goals, and archived decisions. It is not a code-review target. Do not review or comment on plan-file changes in this PR.

The goal, non-goals, and archived decisions are locked. Do not rewrite, expand, or overturn them. Code that violates them may be a finding. Only new source evidence that disproves a decision's factual premise may produce a one-line decision-premise challenge for the developer.

Before investigation, establish each candidate's relationship to the goal and its concrete consequence. Without that relationship, stop all other-dimension investigation, do not write a probe, and do not delegate. A committed out-of-goal slice may still receive only the required dimension 6 scope comment.

IF TRIAGE_LEDGER != none:
  read every comma-separated ledger from oldest to newest;
  for the same location and semantic root cause, the newest disposition wins:
    rejected -> do not re-raise unless new evidence disproves its factual premise
    applied  -> recheck goal relevance, then verify the repair; prior adoption never expands the goal
    flagged  -> do not duplicate as a finding

========== 4. POSTURE ==========

IF POSTURE == devils-advocate:
  actively falsify claims as defined by guide.md without changing scope, lane, MODE, or evidence standards.
ELSE:
  use standard posture.

========== 5. MODE = full ==========

IF MODE == full:
  inspect the non-plan diff, partition goal-serving and out-of-goal slices, and read every eligible goal-serving touched file completely. For out-of-goal slices, read only enough to establish purpose, cumulative size, and paths for dimension 6. Read direct callers, tests, and architecture sources of truth only as needed for the goal-related judgment.

  IF runtime supports subagent dispatch:
    invoking this skill authorizes narrow fan-out for this round, with write access still limited to assigned test files.
    Use the fewest groups justified by diff size:
      structure = dimensions 1, 6, 8
      correctness = dimensions 2, 3, 7
      hygiene = dimensions 4, 5, 9
    Small diffs may combine groups or remain serial.

    Every subagent must:
      - Read(docs/guides/code-review-guide.md)
      - review only assigned dimensions
      - Read(DIFF_SNAPSHOT), complete eligible touched files, and inspect out-of-goal slices only enough for dimension 6 classification
      - obey the verification boundary; only correctness writes and narrowly runs probes
      - use test-file ownership assigned by the main agent and semantic test names without lane/dimension/round markers
      - receive the locked goal, non-goals, archived decisions, and a goal-related narrow assignment; only structure may classify out-of-goal diff
      - return raw candidates only:
        path:line / dimension / observation / evidence / impact / optional remediation

    One checklist-free fresh-eyes subagent is optional.
    Every source produces candidates, never an independent review.md.
    The main agent re-verifies source and goal relationship, deduplicates by location and root cause, removes non-dimension-6 out-of-goal items and nits, and closes all subagents before reporting.
  ELSE:
    the main agent covers all nine dimensions serially.

  GOTO REPORT

========== 6. MODE = incremental ==========

IF MODE == incremental:
  prev_reviews = Read every file in PREV_REVIEWS
  delta = Read(DIFF_DELTA)

  FOR EACH prior-round finding not yet closed:
    reread current source and delta and recheck goal relevance;
    withdraw the reviewer's own out-of-goal item and record a sound author scope rejection as rejected;
    record exactly one reconciliation state under `前轮问题核销`:
      satisfactory | rejected | withdrawn |
      partially-addressed | not-addressed | disputed
    for partially-addressed / not-addressed / disputed:
      restate the same root cause under `新问题与建议`, with `(承 R<round>-<number>)` in the title.

  Classify delta hunks for goal relevance and review eligible changes with Round 1 rigor. For out-of-goal delta, perform only dimension 6 classification. If it expands an existing secondary purpose, size the cumulative slice rather than the current hunk alone. A remediation delta does not independently authorize deeper review of an incidental purpose.

  CLOSED WORLD after goal admission:
    a new finding may originate only from:
      1. an unresolved prior finding
      2. the current delta
      3. a contract contradiction or regression directly caused by the delta
      4. Round 1 coverage explicitly recorded as incomplete in the prior ledger
    do not rescan covered unchanged code;
    do not raise new findings against pre-existing code outside the delta;
    do not rerun full fresh-eyes or full-dimension fan-out.

  IF runtime supports subagent dispatch AND narrow help is useful:
    use the same goal context and admission rules, limited to eligible prior findings, delta, direct contract dependencies, and explicitly uncovered areas;
    collect, deduplicate, and close the subagents.

========== 7. REPORT ==========

REPORT:
  Recheck every candidate's goal relationship and consequence. Out-of-goal committed diff is classified only under dimension 6; do not output other out-of-scope observations.
  Every finding is an evidence-backed fact:
    location / dimension / observation / evidence / impact / optional remediation
  Do not attach severity, blocking, adoption recommendations, or source-agent labels.
  Put substantive problems in `新问题与建议`. Put wording-only consistency drift and dimension 6 `XS`/`S` slices in `同步清单`. A dimension 6 `M` or larger slice belongs in `新问题与建议` and may suggest `split-pr`.
  When a section is empty, write only `无。`.

  Verdict:
    Ready:
      no admitted finding blocks readiness, every prior finding is closed, and goal-related coverage is complete under the guide's proof rules
    Needs Refinement:
      a locally repairable substantive problem exists, or dimension 6 found an out-of-goal slice of size M or larger; never escalate because of round count
    Abandon:
      the architecture is fundamentally wrong or cannot be repaired within the current diff; dimension 6 alone never yields Abandon

  Before completion, Read(docs/guides/review-format.md) completely.
  Write <round-NN/review.md> using the PR-review template exactly.

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

STOP. The developer decides merging, closing, and whether to adopt findings.
```
