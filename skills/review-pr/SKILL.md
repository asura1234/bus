---
name: review-pr
description: Iterative code review in which the reviewer proves suspicions in the correctness / security / bug dimensions by writing tests that run red (write access is limited to test files; all other dimensions are read-only). Round 1 fully reviews the current branch's committed diff against base, excluding plans/; Round 2+ only reconciles the prior round and reviews this round's delta and its direct consequences. Supports stable reviewer lanes, an optional associated plan, author triage ledgers, adversarial posture, and narrow subagent fan-out; the final review.md is deterministically rendered by a script into the chat reply. Use when the user asks to review a PR, review code, or re-review the current branch.
---

Review principles are in [guide.md](./guide.md); the SOT for the 9 review dimensions, severity, and guardrails is
[code-review-guide.md](../../docs/guides/code-review-guide.md). This file defines only the executable flow.

```text
INPUT $ARGUMENTS =
  [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan-file>]

DEFAULT base = origin/master
DEFAULT reviewer = default (only when the prologue can select a lane unambiguously)

========== 1. VERIFICATION BOUNDARY ==========

The proof obligation, probe lifecycle, and the criterion for "unproven" all live in guide.md
"Write behavioral suspicion as a test first, then as a finding"; this section defines only **permissions** —
they must be visible before acting, and when they fail they must fail on the safe side.

MAY:
  - write this skill's round artifacts under temp/review-pr/; the Goal/Non-goals locks come only
    from skills/pr/scripts/pr_goal_context.py
  - write test files: create new ones, or **add** your own cases to existing test files
    (only dimensions 2 correctness / 3 security / 7 bug need this; other dimensions judge source read-only)
  - run read-only queries: git, grep, reading files
  - run tests narrowly: Rust with `just test-one <filter>`, a skill's Python tests with
    `python3 -m pytest <test file>::<test name>`

MUST NOT:
  - modify, create, or delete any **non-test** file (production code, config, build scripts, docs, plan files)
  - change the implementation or roll back code to make a test red — that manufactures evidence, and the hypothesis fails
  - delete or weaken someone else's probe or existing assertions
  - git add / commit / checkout / stash / rebase
  - run formatters, lint fixers, or other write commands
  - run quality gates: `just lint`, `cargo fmt --check`, `just check`, `just ci`, `just build`,
    **including** filtered forms. Those belong to CI and gate-and-fix
  - run the unfiltered full test suite: `just test`, or `cargo nextest run` without a filter;
    E2E does not enter the review side

========== 2. DETERMINISTIC PROLOGUE ==========

Run:
  python3 skills/review-pr/scripts/review_round.py \
    [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan-file>]

IF exit != 0 AND output says multiple / named lanes make a bare invocation ambiguous:
  IF the developer supplied a lane, or said "use the previous lane" and this conversation has the REVIEWER of a prior successful prologue:
    that value must exactly equal an existing lane listed by this run's FAIL; approximate names must first be confirmed with the developer.
    Add `--reviewer <lane>` with that exact lane value and rerun the prologue immediately.
    Do not reply with only the lane name, and do not invoke bare again.
  ELSE:
    relay stdout/stderr verbatim, ask the developer for the exact lane, then STOP.
ELSE IF exit != 0 AND FAIL names missing, blank, or mismatched locked Goal/Non-goals:
  IF --plan was supplied:
    run pr_goal_context.py --branch <branch> --output temp/review-pr/goal-context.md --plan <plan-file>;
    rerun the prologue once; relay any remaining FAIL verbatim and STOP.
  ELSE:
    ask the developer for the PR's one-sentence single Goal and its explicit Non-goals (`无` when none);
    write each reply verbatim to ignored temp goal and non-goal files.
    Trim only leading/trailing whitespace; do not summarize, rewrite, or infer from the diff/commits/PR description.
    run pr_goal_context.py --branch <branch> --output temp/review-pr/goal-context.md --goal-file <goal-file> --non-goal-file <non-goal-file>;
    rerun the prologue.
ELSE IF exit != 0:
  relay stdout/stderr verbatim, then STOP.

IF NOTE says the working tree has uncommitted changes:
  first tell the developer this round reviews only the committed diff.
  Probe tests written this round likewise stay only in the working tree and do not enter diff scope, finding counts, or the verdict.

Capture:
  ROUND, MODE, POSTURE, REVIEWER, BRANCH, BASE, HEAD, STATE_DIR,
  DIFF_SNAPSHOT, DIFF_DELTA, PREV_REVIEWS, PLAN, LOCKED_GOAL_FILE,
  LOCKED_NON_GOALS_FILE, TRIAGE_LEDGER

`DIFF_SNAPSHOT` / `DIFF_DELTA` already mechanically exclude `plans/**` in the prologue. Even when a plan file
changes in the committed branch diff, it does not enter touched-file scope, findings, the sync list, or the PR single-purpose judgment.

========== 3. LOAD LOCKED CONTEXT ==========

Read(skills/review-pr/guide.md) completely
Read(docs/guides/code-review-guide.md) completely
Read(docs/guides/architecture-principles.md) completely
diff = Read(DIFF_SNAPSHOT)

IF PLAN != none:
  plan = Read(PLAN) completely
  locked_goal = the plan's `## 目标` verbatim, equal to LOCKED_GOAL_FILE
  non_goals = the plan's `## 非目标` verbatim, equal to LOCKED_NON_GOALS_FILE
  archived_decisions = the plan's `## 已归档的决策` verbatim (may be absent)
ELSE:
  locked_goal = Read(LOCKED_GOAL_FILE)
  non_goals = Read(LOCKED_NON_GOALS_FILE)
  archived_decisions = none

The associated plan serves only as locked goal / non-goals / archived decisions input; it is not a
code-review target. Do not review or comment on the plan's own changes in this PR.

The goal, non-goals, and archived decisions are locked inputs; do not rewrite, expand, or overturn them.
Code that violates the locked inputs may become a finding; only when new evidence disproves a decision's factual premise
may you output one "decision-premise challenge" sentence for the developer to rule on.

Before investigating, establish goal relevance and a concrete consequence per code-review-guide "goal-relevance admission", noted briefly in the candidate's "impact".
Without relevance, stop investigating other dimensions, write no probe, and do not delegate; if it is in the committed diff, still output
a scope comment under dimension 6, and beyond that leave at most one observation sentence in the file-only exploration record.

IF TRIAGE_LEDGER != none:
  decided = Read every comma-separated TRIAGE_LEDGER, oldest to newest
  For the same location + semantic root cause, the newest disposition wins:
    rejected -> do not re-raise unless new evidence disproves its factual premise
    applied  -> recheck goal relevance first, then verify the repair; prior adoption does not expand the goal
    flagged  -> do not duplicate as a finding

========== 4. POSTURE ==========

IF POSTURE == devils-advocate:
  actively falsify per guide.md's adversarial posture, without changing scope, lane, MODE, or the proof standard.
ELSE:
  use the standard posture.

========== 5. MODE = full ==========

IF MODE == full:
  Inspect the non-plan diff, partition goal-serving and out-of-goal slices, and read every eligible
  goal-serving touched file in full. For out-of-goal slices, inspect only enough evidence to establish
  purpose, cumulative size, and affected paths for dimension 6.
  Use only direct callers, tests, and applicable architecture sources of truth as the upstream/downstream evidence needed for judgment;
  untouched modules with no direct contract relationship do not enter finding scope.

  IF runtime supports subagent dispatch:
    The user invoking this skill explicitly authorizes narrow fan-out for this round (write permission as in §1, limited to test files).
    Split the 9 dimensions into the fewest necessary groups by diff size:
      structure = 1, 6, 8
      correctness = 2, 3, 7
      hygiene = 4, 5, 9
    At most one subagent per group; small diffs may merge groups or be done serially by the main agent.
    Every **dimension-group** subagent must (except the fresh-eyes subagent, see below):
      - Read(docs/guides/code-review-guide.md) and docs/guides/architecture-principles.md
      - review only its assigned dimensions
      - Read(DIFF_SNAPSHOT); read the full current state of in-goal touched files; for dimension 6's out-of-goal slices only judge purpose / size
      - obey §1 VERIFICATION BOUNDARY: only the correctness group writes a suspicion as a test and runs it narrowly
        before deciding whether to report, writing only test files and running no quality gates; structure / hygiene groups are read-only
      - the main agent first assigns non-overlapping test files; name them semantically, never putting lane, dimension group, or round into test names
      - the main agent passes locked_goal / non_goals / archived_decisions and a goal-related narrow task; the structure group additionally
        does only dimension 6 classification on out-of-goal diff; other groups stop investigating on any unrelated item
      - return raw candidates: path:line / dimension / observation / evidence / impact / optional remediation

    **You must also dispatch one fresh-eyes subagent** (rationale in guide.md "Fan-out increases coverage"):
      - its ground truth is **verbatim identical** to the other groups: the same DIFF_SNAPSHOT, the same locked_goal /
        non_goals. The only allowed difference is "no checklist"
      - it **must** read guide.md "Write behavioral suspicion as a test first, then as a finding" — the proof boundary is written only there;
        not reading it means being unconstrained, and what gets loosened is exactly the rule that most needs to bind fresh-eyes
      - it **must not** read the rest of guide.md, code-review-guide.md, architecture-principles.md,
        the dimension list, or any prior-round review
      - §1 and the guide's proof boundary apply to it **in full**, without loosening a single word
    Claude Code may additionally use the built-in /code-review as a candidate source.
    All sources produce only candidates, never each their own review.md.
    The main agent re-verifies each against source and goal relevance, deduplicates by location + semantic root cause, discards out-of-scope items outside dimension 6 and nitpicks,
    then closes all subagents of this round.
  ELSE:
    The main agent covers the 9 dimensions serially.

  GOTO REPORT

========== 6. MODE = incremental ==========

IF MODE == incremental:
  prev_reviews = Read every file in PREV_REVIEWS
  delta = Read(DIFF_DELTA)

  FOR EACH prior-round finding not yet closed:
    reread current source and delta, recheck goal relevance first; your own out-of-scope items become withdrawn, an accepted author scope rejection becomes rejected.
    Record exactly one state under `前轮问题核销`:
      satisfactory | rejected | withdrawn |
      partially-addressed | not-addressed | disputed
    partially-addressed / not-addressed / disputed:
      restate the same root cause under `新问题与建议`, with the title carrying `承 R<round>-<number>`;
      do not disguise it as a new problem under a new title.

  Inspect the delta for goal relevance; review eligible changes with Round 1 rigor. For out-of-goal delta
  only do dimension 6 classification; if it expands an existing incidental purpose, recompute size over that purpose's cumulative slice, not just this round's hunks.
  An incidental-repair delta does not independently authorize further investigation or findings in other dimensions.

  CLOSED WORLD:
    After goal-relevance admission, only these sources may enter this round's new findings:
      1. unclosed prior-round findings
      2. directly introduced by this round's delta
      3. contract contradictions or regressions directly caused by the delta changes
      4. Round 1 coverage explicitly marked incomplete in the prior ledger
    Do not rescan covered, unchanged code; do not raise brand-new findings against existing code outside the delta;
    do not rerun full fresh-eyes or full-dimension fan-out.

  IF runtime supports subagent dispatch AND narrow help is useful:
    with the same goal context and admission requirements as full mode, dispatch only to eligible prior items, the delta, direct contract dependencies, and explicitly uncovered areas.
    Collect, deduplicate, and close the subagents.

========== 7. REPORT ==========

REPORT:
  Recheck every candidate's goal relevance and consequence; out-of-goal committed diff is classified only under dimension 6, and other out-of-scope observations are not output.
  Every finding must be an evidence-backed fact:
    location / dimension / observation / evidence / impact / optional possible remediation
  Do not attach severity, blocking, adoption recommendations, or source-agent labels to findings.
  Substantive problems go into `新问题与建议`; wording consistency drift and dimension 6 `XS` / `S` out-of-goal slices go into
  `同步清单`. Dimension 6 slices of `M` or larger go into new problems, and the possible remediation uses `split-pr`.
  When there is no finding, the corresponding section reads `无。`.

  Verdict:
    Ready:
      no qualifying finding blocks Ready, every prior problem is closed, and goal-related coverage is complete (per the guide's proof rules)
    Needs Refinement:
      a locally repairable substantive problem exists, or a dimension 6 out-of-goal slice of `M` or larger; never escalate because of round count
    Abandon:
      fundamental architecture error or not locally salvageable; dimension 6 alone never yields Abandon

  Before completion, Read(docs/guides/review-format.md) completely.
  Write <round-NN/review.md> strictly per the PR template.

========== 8. DETERMINISTIC RESPONSE GATE ==========

Run:
  python3 cli_extensions/review_artifact.py render-response \
    --review-file <round-NN/review.md> \
    --output <round-NN/chat-response.md>

IF exit != 0:
  fix review.md and rerun; never report a verdict first.

Read(chat-response.md) completely.
The final response MUST equal chat-response.md verbatim.
Do not hand-summarize, rewrite, add a preface, add links, add a closing, or change the verdict.

STOP. Merging, closing the PR, and whether to adopt findings are all decided by the developer.
```
