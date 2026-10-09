---
name: review-pr
description: Iterative code review in which the reviewer proves suspicions in the correctness / security / bug dimensions by writing tests that run red (write access is limited to test files; all other dimensions are read-only). Round 1 fully reviews the current branch's committed diff against base, excluding plans/, or a fixed chunk's whole committed files with --scope; Round 2+ only reconciles the prior round and reviews this round's delta and its direct consequences. Supports stable reviewer lanes, an optional associated plan, author triage ledgers, adversarial posture, and narrow subagent fan-out; the final review.md is deterministically rendered by a script into the chat reply. Use when the user asks to review a PR, production chunk, review code, or re-review the current branch.
---

Review principles are in [guide.md](./guide.md); the SOT for the 9 review dimensions, severity, and guardrails is
[code-review-guide.md](../../docs/guides/code-review-guide.md). This file defines only the executable flow.

```text
INPUT $ARGUMENTS =
  [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan-file>]
  [--scope <FILE_LIST|chunk.json>]

DEFAULT base = origin/master
DEFAULT reviewer = default (only when the prologue can select a lane unambiguously)
DEFAULT target = branch (unchanged committed diff against base)

--scope selects a fixed whole-file chunk, not a filter over the branch diff.
FILE_LIST is UTF-8, one repo-relative file path per line; blank lines and # comments are ignored.
Alternatively, a JSON manifest declares the full file set and exact probe write allowlist:
  {"files":["src/example.rs","src/example_test.rs"],"test_files":["src/example_test.rs"]}
Dedicated *_test files in FILE_LIST (or files without explicit test_files) form the probe allowlist.
Explicit test_files may include new, not-yet-committed *_test paths; they become part of the chunk.
All other files must exist as ordinary UTF-8 committed files in HEAD on Round 1.
No directories, globs, binary files, symlinks, plans/ (including docs/plans/), or temp artifacts; paths are repo-relative
(absolute paths within this repo are also accepted), independent of the manifest's location.
The canonical file set + test allowlist determines SCOPE_HASH; reordering / duplicates / manifest
location do not change identity. Changing membership or permissions starts a separate full review.
Use the same manifest and reviewer lane for continuation. A chunk still uses the named feature
branch, resolvable base, and exact branch-level Goal/Non-goals locks; no branch diff is required.
For production review the developer's locked Goal must authorize reviewing the entire chunk.

========== 1. VERIFICATION BOUNDARY ==========

The proof obligation, probe lifecycle, and the criterion for "unproven" all live in guide.md
"Write behavioral suspicion as a test first, then as a finding"; this section defines only **permissions** —
they must be visible before acting, and when they fail they must fail on the safe side.

MAY:
  - write this skill's round artifacts under temp/review-pr/; the Goal/Non-goals locks come only
    from skills/pr/scripts/pr_goal_context.py
  - write test files: create new ones, or **add** your own cases to existing test files
    (only dimensions 2 correctness / 3 security / 7 bug need this; other dimensions judge source read-only)
    In scope mode, write only the exact SCOPE_TEST_FILES paths; no inline production-file edits,
    no new test path outside the manifest, and no dependency/config wiring edits.
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
    [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan-file>] \
    [--scope <FILE_LIST|chunk.json>]

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
  first tell the developer this round reviews only committed code (branch diff or whole chunk files).
  Probe tests written this round likewise stay only in the working tree and do not enter the snapshot or author delta.
  Red probes remain finding evidence; they are not themselves reviewed author changes.

Capture:
  ROUND, MODE, POSTURE, REVIEWER, BRANCH, BASE, HEAD, STATE_DIR,
  DIFF_SNAPSHOT, DIFF_DELTA, PREV_REVIEWS, PLAN, LOCKED_GOAL_FILE,
  LOCKED_NON_GOALS_FILE, TRIAGE_LEDGER
  For --scope also capture TARGET_KIND=scope, SCOPE_HASH, SCOPE_FILE, SCOPE_SNAPSHOT, SCOPE_TEST_FILES;
  without --scope, TARGET_KIND is branch and these extra fields are absent.

`DIFF_SNAPSHOT` / `DIFF_DELTA` already mechanically exclude `plans/**` in the prologue. Even when a plan file
changes in the committed branch diff, it does not enter touched-file scope, findings, the sync list, or the PR single-purpose judgment.

In scope mode STATE_DIR is temp/review-pr/<branch_slug>/scopes/<SCOPE_HASH>/<reviewer>/.
SCOPE_FILE is the canonical scope.json shared by those lanes; SCOPE_SNAPSHOT stores complete file
contents, modes, and the pinned HEAD. DIFF_SNAPSHOT shows every line of those committed files;
read the complete files from SCOPE_SNAPSHOT rather than dirty working-tree source.
Round 2+ DIFF_DELTA is a per-file diff against this lane's previous completed snapshot, including
adopted tests and deletions. Unchanged content yields an empty delta even after a rebase.
Missing/corrupt prior snapshots fail closed. PREV_REVIEWS and TRIAGE_LEDGER are confined to this
branch + chunk; branch reviews, other chunks, and other reviewer histories never enter the lane.

========== 3. LOAD LOCKED CONTEXT ==========

Read(skills/review-pr/guide.md) completely
Read(docs/guides/code-review-guide.md) completely
Read(docs/guides/architecture-principles.md) completely
IF TARGET_KIND == scope:
  Read(SCOPE_FILE).
  IF MODE == full: Read(SCOPE_SNAPSHOT) completely.
  ELSE: Read(DIFF_DELTA); read snapshot file contents only for eligible prior items / changes / direct consequences.
ELSE: diff = Read(DIFF_SNAPSHOT)

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
In scope mode the admitted target is every line of the chunk's whole files, including unchanged
production code, under the locked production-review Goal. Do not narrow it to the feature diff.
Outside files are read-only direct-contract context; findings reference chunk files. Dimension 6
checks the declared assignment's boundary; historical code is not a new incidental PR purpose.

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
  Read(skills/review-pr/scripts/full_review.md) completely and execute its procedure.
  Its GOTO REPORT continues at section 7 below.

========== 6. MODE = incremental ==========

IF MODE == incremental:
  prev_reviews = Read every file in PREV_REVIEWS
  delta = Read(DIFF_DELTA)
  In scope mode use only the chunk's author delta since this lane's previous completed round,
  plus direct consequences and prior unresolved / explicitly unfinished coverage. Do not start
  another full-file sweep merely because the target is a production chunk. Keep probes within SCOPE_TEST_FILES.

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
  In scope mode add **范围哈希**：<SCOPE_HASH> and **范围文件**：<SCOPE_FILE> to the header;
  the existing sections, reconciliation table, finding format, and verdicts stay the same.

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
