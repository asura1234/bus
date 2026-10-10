---
name: merge-pr
description: Watch an open PR's two signals until it can merge — CI conclusions and review comments. Attribute every red CI check individually; failures caused by this PR's changes must be fixed (reproduce by running the gates locally, never by reading log tails), while pre-existing failures from the base or flakes may be carried into the merge. Handle only the latest round of review comments, hand them to address-review-comments, and post its triage ledger back to the PR verbatim (otherwise REJECTs leave no trace and machine review raises the same points again next round). When the branch has diverged from origin/master, converge before talking about merging — rebase-origin-main by default, and merge master in instead when the commit count exceeds a threshold (this repo squash-merges, so branch-internal history is discarded anyway). Squash-merge when all green; when only red checks not caused by this PR remain, admin-merge and state the release basis for each one. Use when the user asks to merge a PR, watch CI, or asks "can this merge yet".
---

# Merge PR

Read [guide.md](guide.md) completely: the three acceptable kinds of attribution evidence, why log
tails must not be read, and why only the latest comment round is handled all live there. Without it this skill will release red checks it caused itself.

```text
INPUT = [<pr-number>] [--no-merge] [--poll-seconds N (default 30)] [--max-wait N (default 1800)]
        [--rebase-max-commits N (default 20)] [--hold-before-merge]

repo   = git rev-parse --show-toplevel
branch = git branch --show-current
pr     = the number the caller gave; when omitted, take the current branch's PR via `gh pr view --json number`

IF not in a Git repository OR branch is empty OR branch IN {main, master}
  STOP "merge-pr runs only on a feature branch."

========== 1. Take the signals ==========

Run:
  python3 skills/merge-pr/scripts/pr_signals.py checks --pr <pr>
  python3 skills/merge-pr/scripts/pr_signals.py scope  --pr <pr>

Capture: HEAD, branch, base, isDraft, mergeable, mergeStateStatus, checkCount,
pending / passed / failed / **other**, plus this PR's `topLevel` and `files`.
scope is the first-hand basis for every attribution below; get it first.

-- This section takes a single snapshot only; waiting belongs to section 3, and must come after review handling (guide.md).

The source of truth for `checks` is `statusCheckRollup`, at **per-check** granularity; do not switch back to `gh run list`
(reasons in guide.md "Per check, not per workflow").

IF checks reports `isDraft == true`
  Run: `gh pr ready <pr>`, then return to this section and retake the signals; continue only after confirming `isDraft == false`.
  `gh pr ready` fails (insufficient permission, PR already closed) → STOP and report verbatim; do not skip past it.
-- Reasons in guide.md "Draft becomes ready first, no stop".
-- `checkCount == 0` does not STOP here: the 0 in the tens of seconds right after a push is temporary; the later wait handles it.

========== 1b. Converge first when diverged from origin/master ==========

The entry is a **positive enumeration**; only these two states enter this section. It must not be written as a negation (reasons in guide.md "Why the entry must be a positive
enumeration"):

  divergent = (mergeable == "CONFLICTING")
              OR (mergeable == "MERGEABLE" AND mergeStateStatus == "BEHIND")

IF NOT divergent
  Skip this section.

The worktree must be clean (`git status --porcelain --untracked-files=all` is empty); otherwise first hand it to
`commit-and-push` to land it, or let the developer deal with it.

ahead = `git rev-list --count origin/master..HEAD`

IF mergeStateStatus == "BEHIND":        Invoke `rebase-origin-main` (pure replay, no conflicts)
ELSE IF ahead <= rebase_max_commits:    Invoke `rebase-origin-main` (necessarily CONFLICTING here)
ELSE:                                   `git fetch origin master` + `git merge origin/master`
  Conflicts follow the same tiered handling as `rebase-origin-main`: shared infra (`Cargo.lock`)
  follows its shared-infra rule, and any branch change it discards is recorded in `dropped_shared_infra`; minor conflicts are resolved automatically;
  **major conflicts go to the developer**. After resolving, Invoke `commit-and-push` (route rationale in guide.md "Merge method").

IF the invoked skill asks the developer to rule on a major conflict
  STOP and relay it verbatim.
IF `dropped_shared_infra` is nonempty
  List each entry: this branch's changes to the lockfile were dropped; this must not pass silently.

Return to section 1 and retake the signals.
-- Rebase force-pushes: every old review anchor is invalidated and all CI reruns; enter this section only when truly diverged.

========== 1c. Other non-mergeable states ==========

IF mergeable == "UNKNOWN":
  This is the normal state after every push, not unmergeable. Sleep poll_seconds and return to section 1 to retake; STOP only when it is
  still `UNKNOWN` at max_wait. The only correct action is **to ask again** (guide.md "Why the entry must be a positive enumeration").

IF mergeable != "MERGEABLE"
  STOP and report mergeStateStatus. Non-mergeable states section 1b cannot handle go to the developer.

========== 2. Clear review handling first ==========

**Review before CI**: fixes from review advance HEAD and invalidate the running CI, not the other way around (see guide.md).

Run:
  python3 skills/address-review-comments/scripts/github_review_lane.py reviews --pr <pr>

WHILE `everReviewed` AND NOT `coveredAtHead` AND waited < max_wait:
  Sleep poll_seconds and rerun.
-- The semantics of the three signals and why the wait condition must be `NOT coveredAtHead` are in guide.md "The two
--   signals of review coverage". `everReviewed == false` does not wait here (machine review may not be wired at all); report it as is.

IF waited >= max_wait AND NOT `coveredAtHead`
  **STOP**, and say which signal was not met: if `pendingReviewers` is nonempty, report the commit each one stopped at;
  if it is empty and `headHasReview` is false, report "no review event on the current HEAD" with `reviewedOnceAt`.
  Continue only after the developer explicitly confirms; it must truly stop.

Invoke `address-review-comments pr --github --pr <pr>`

Fetching comments, determining rounds, adjudicating, fixing, landing and posting the ledger **all belong to it**; this skill does none of those steps for it.
The reason `--github` uses only this one lane is in guide.md "Why only the github lane".

Handle its return in four ways:
  FLAG                         → STOP and relay verbatim. By definition a FLAG needs the developer's call.
  landed fixes                 → return to section 1 and retake the signals (HEAD changed, CI must rerun).
  STOP because anchor is not the current HEAD → relay verbatim and wait for the review of the latest HEAD; never treat an old round as reviewed.
  `no-comments`                → nothing to do in this section; continue. A clean review is not a reason to stop.

Finishing one round **does not mean** the reviewer agreed. `triaged` is a marker in the local ledger meaning the author did the homework; whether the reviewer
agrees depends on its own verdict. Run:

  python3 skills/address-review-comments/scripts/github_review_lane.py verdict --pr <pr>

The `verdict` output is **informational only**, not a release condition: author and reviewer are peers, and a rejection with first-hand counter-evidence is an equally valid
final outcome. The convergence criterion is "the author has adjudicated every point the reviewer made on the current HEAD", judged by `fetch` — true when no new
round is opened.

  - This round landed fixes → return to section 1. The push triggers a re-review; the next verdict is the conclusion on the new tree.
  - This round was all REJECT (each with first-hand counter-evidence in the ledger) → continue. A verdict sitting at 🟡 / 🔵 is the **expected result**, not a blocker.
  - An unhandled FLAG → STOP. That is the kind the author cannot converge on and explicitly needs the developer's call; it has nothing to do with rejection.
-- Why 🟢 cannot be a release condition, and why this path does not need `--admin`, are in guide.md
--   "The merge gate is 'the author adjudicated', not 'the reviewer agreed'".

-- Sections 2/5 advance HEAD and therefore loop; after two consecutive rounds without any progress, STOP and report.
-- **The convergence criterion is "nothing left to handle", not "someone reviewed"**; the loop must include one `fetch` (guide.md).

========== 3. Wait for CI to converge (wait last) ==========

By here review covers the current HEAD, so this section waits on the CI of **the tree that will be merged**.

WHILE (`checkCount == 0` OR pending is nonempty) AND waited < max_wait:
  Sleep poll_seconds and rerun `checks`.
  -- Do not change the repository while waiting: once the tree moves, the running checks no longer verify the tree to be merged.

-- The convergence condition is "`checkCount > 0` **and** pending is empty"; both are required.
--   **Zero checks is not all green; it is no signal**; measurements in guide.md "CI convergence criterion".

IF waited >= max_wait AND pending is still nonempty
  STOP and report the jobs still running. Do not treat "not finished yet" as green.
IF waited >= max_wait AND `checkCount == 0`
  STOP: no check registered even by timeout means this PR never triggered the gates, which needs a human look.

========== 4. Attribute each red CI check ==========

Every entry in `failed` and `other` must go through this section. `other` holds conclusions that "completed but are neither SUCCESS nor on the failure
list" (cancelled / skipped / neutral / stale…): not green, just not shaped like red.

FOR EACH job IN failed + other:
  a. Take that check's history (`--job` takes the **per-check name** from `checks`, not the workflow name):
       python3 skills/merge-pr/scripts/pr_signals.py history --pr <pr> --job "<name>"
     Green earlier, red now → **strong hint this PR caused it**; go straight to (c), do not look for environmental excuses first.
  b. Take the failure evidence: use `failed[].logCommand`. **The log may be truncated before the real failure** (see guide).
  c. Decide per guide "Three acceptable kinds of evidence"; each red check lands in exactly one class: IN-SCOPE (caused by this PR, must fix) /
     PRE-EXISTING / FLAKY / ENVIRONMENT. Anything undecidable is IN-SCOPE; never release by default.
  d. When FLAKY is suspected, **rerun** instead of reasoning: `gh run rerun <runId> --failed`, then return to section 2 to wait for convergence.

========== 5. Fix IN-SCOPE red checks ==========

IF any IN-SCOPE red check exists:
  The worktree must be clean (`git status --porcelain --untracked-files=all` is empty); otherwise first hand it to
  `commit-and-push` to land it, or let the developer deal with it.

  Invoke `gate-and-fix --base <merge-base>`. **This is the only thing entitled to claim "the gates are green"** —
  hand-running a few scoped commands does not count (reasons in guide.md "Why attribution must gather evidence mechanically").

  IF gate-and-fix STOPs or cannot converge
    STOP and report the artifact and blocker.
  IF the fix exceeds this PR's locked goal
    STOP and hand it to the developer to rule; do not widen scope to turn CI green.

  Return to section 1 and retake the signals (HEAD has changed; review must cover it again).

========== 6. Merge ==========

IF --no-merge   -- stops before the retake and assertions; --hold-before-merge stops after all of them, right before the merge command
  Report the current state and conclusion, STOP. Do not merge.

First **retake** the review lane; do not assert from section 2's conclusion (reasons in guide.md "Review must be retaken before merging"):

Run:
  python3 skills/address-review-comments/scripts/github_review_lane.py fetch   --pr <pr>
  python3 skills/address-review-comments/scripts/github_review_lane.py reviews --pr <pr>

Run the verdict once more too (for the report, not as a release condition):
  python3 skills/address-review-comments/scripts/github_review_lane.py verdict --pr <pr>

**Branch first, then assert**:

IF this `fetch` returns `status == head-moved`
  Return to **section 1** and retake everything: a racy snapshot would merge with stale CI, review state and `merge_head`.

IF this `fetch` opened a round to handle (`status == created`, or the latest round has `triaged == false`)
  Return to section 2 to handle it; do not STOP here.
-- The order cannot be reversed; reasons in guide.md "Branch first, then assert".

Assert: pending is empty; every entry in `failed` and `other` is attributed and **none is IN-SCOPE**;
no unhandled FLAG; mergeable == "MERGEABLE"; isDraft == false;
**review covers the current HEAD** (`coveredAtHead == true`, or the developer explicitly confirmed release);
**and this `fetch` opened no round to handle** (`no-comments`, or the latest round has `triaged == true`).
-- This last one is the criterion for "the review path has no unadjudicated feedback": anything new the reviewer says on the current HEAD opens a
--   new round, and if none opens, every point has entered the ledger. It **does not require the reviewer to agree**, only that the author adjudicated.
IF `verdict.approved == false`
  Merge as usual, but state in the output: the verdict text at that time, the count of unresolved findings, and each one's ledger disposition
  and first-hand counter-evidence. A review sitting at 🟡 / 🔵 with all findings REJECTed is the expected end state, not a release exception; **do not use `--admin`**.
-- The criterion is `triaged`, not `reviewExists`; see guide.md "The criterion that a round is done".
-- A nonempty `other` is not released just because it is not called failed: a cancelled required check proved nothing either,
--   and must be attributed exactly like a red check, otherwise STOP.

merge_head = the HEAD last taken in section 1
-- Read the **full 40-character sha** with `git rev-parse`; never hand-complete it (guide.md "merge_head must be read, not hand-written").
IF --hold-before-merge
  Report "ready to merge at <merge_head>, holding for developer verification" and the state summary (guide.md "Holding before merge"), STOP. Do not merge.

IF failed and other are both empty:
  gh pr merge <pr> --squash --delete-branch=false --match-head-commit <merge_head>
ELSE:
  -- Only PRE-EXISTING / FLAKY / ENVIRONMENT remain; state the release basis for each one,
  --   no unnamed summaries like "the rest are environment issues".
  gh pr merge <pr> --squash --admin --delete-branch=false --match-head-commit <merge_head>

-- `--match-head-commit` is not optional: without it a push after the signals were taken merges a tree that was never verified.

Read back: `gh pr view <pr> --json state,mergedAt,mergeCommit`
IF state != "MERGED"
  STOP and report the actual fields. Never announce a merge from a command's exit code.

After a successful merge, append one line of review metrics:

  python3 skills/merge-pr/scripts/review_metrics.py --pr <pr> \
    --applied <N> --rejected <N> [--flagged <N>] --verdict "<verdict text at merge time>"

Review rounds, finding count, replied count and diff size are taken by the script itself; adjudication counts have no machine source of truth, fill them in
**truthfully** from this run's ledger, and if unsure do not fill in a flattering number.
-- Why this ledger exists, and why it lands in `temp/`, see guide.md "Review metrics".
-- A failed append does not roll back the merge or change the output conclusion; report it as is.

========== OUTPUT ==========

Report:
  - PR number, URL, final HEAD, merge commit
  - Number of rounds, and what each round did (which IN-SCOPE red checks were fixed, how many comments were handled)
  - Per-check CI conclusions: passed count; for each red check the job name, attribution class, **and which evidence it rests on**
  - Review handling: which round was handled (anchor commit), counts per disposition, triage path
  - If diverged from origin/master: whether rebase or merge was used, what `ahead` was, and
    each entry of the discarded `dropped_shared_infra` (lockfile)
  - Whether a normal merge or `--admin` was used; with admin, repeat the release basis for each one
  - Unhandled FLAGs (if any) must be the last content of the output
```
