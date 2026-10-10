# Merge-PR guide

`SKILL.md` defines the execution order; this document defines the judgment. There is only one core rule: **a red check is either caused by this PR (must fix)
or not (may merge with it) — and that judgment cannot rest on the job name or the log tail.**

## Per check, not per workflow

The source of truth for `checks` is `statusCheckRollup` — the one GitHub itself uses to decide whether a PR can merge. In this repo the
`CI` workflow carries `conventional-commits` / `check (ubuntu-latest)` / `check (macos-latest)` / `check (windows-latest)` /
`Windows ConPTY package` as independent checks, and `gh run list`'s workflow granularity collapses them into one name, while also listing
workflows such as `Copilot` that do not gate at all.

## Why attribution must gather evidence mechanically

Measured (libtv-desktop), the same mistake twice: `Lint Check` was red on three commits in a row, the log truncated at the `pnpm exec eslint …` line
with nothing after it, so it was judged "the process died, eslint reported nothing" → attributed to infrastructure. The truth was that it did not die in
eslint at all but in a later Python stage: 12 probe test files had 15 `ruff I001`. The truncation hid the real failure
behind it, and "reading the log tail" is inherently useless against a truncated log.

The cost that time was three rounds of red CI waved through as noise. **The corrected criterion is just one: run that gate locally.**
`gate-and-fix` runs the full gate set on a clean committed tree and produces a verifiable artifact; only its PASS is a PASS.

Why hand-running a few scoped commands locally is not enough was hit again on the spot while dogfooding this skill (libtv-desktop):
`./run lint check-files <untracked files>` answered **untracked files** directly with `当前动作无适用工具，本次未执行` ("no applicable tool for this action, not run")
and exited nonzero — it is not "check passed", but it is shaped very much like a run that found no problems. After `git add`, the same files
immediately reported `ruff I001`. The `I001` in those 15 probe files leaked exactly this way: the probes were newly created by the reviewer,
and before they were `git add`ed, every per-file lint was blind to them.

**So the coverage of scoped lint depends on Git state, not on which paths you passed.** That is why it is handed to
`gate-and-fix`: its precondition is a clean committed tree, where untracked files cannot exist at that moment.

Therefore:

- **Do not** decide ownership from the tail of `gh run view --log-failed` alone; truncation is normal.
- **Do not** assume it is an environment problem because the job name contains Windows / macOS / Build / E2E.
- Judging "not caused by this PR" requires one of the pieces of evidence below, written out in the output.

## Three acceptable kinds of "not caused by this PR" evidence

Ordered by strength; one is enough, but you must name which one:

1. **Disjoint scope**: the subtree where the failing thing lives was not touched by a single file in this PR. Use
   `pr_signals.py scope` to get `topLevel` and `files`, then compare with what the failing job actually ran.
   Example: the PR only touches `skills/` and `docs/`, while `check (windows-latest)` fails in `src/platform/windows/`.
   This one is the strongest, but **only holds when the failure is actually localized to a specific subtree** — "the whole repo's lint is red" does not qualify.
2. **History is red too**: the same job was also red on an earlier HEAD that does not contain this change. Use
   `pr_signals.py history --job`. Note the reverse signal must be acknowledged too: earlier **green**, now **red**,
   is a strong hint that "this PR caused it"; do not ignore it because "I think it has nothing to do with me".
3. **Self-described environment failure**: the job exited before running any repository code, and the log clearly points at the host environment.
   Examples: `nodename nor servname provided` (DNS), `spawn EPERM` (runner permissions),
   failing to fetch `actions/checkout` (network). The criterion is **the failure happened in the environment-check stage or the network layer**,
   not "the log mentions network" — real code failures also have network noise beside them.

## Flaky criterion: rerun, don't debate

When flaky is suspected, **rerun that job** (`gh run rerun <id> --failed`); do not reason yourself into it.
A passing rerun means flaky, and it may be carried into the merge; still red after the rerun, re-attribute per the three above, and if it cannot be attributed away it is this PR's.

The combination "green earlier, red now, and our diff is disjoint" is the most deceptive — it has both 1 and reverse 2.
In that case you **must rerun**: disjointness only says "it shouldn't be us", only a rerun can say "it really isn't".

## Handle only the latest round of review comments

Machine review pins comments to the commit that triggered it, and later pushes neither retract nor update them. So after the branch moves forward a few rounds,
the earlier rounds' comments contain many entries **already adjudicated in an earlier handling**. Feeding the whole batch back into
`address-review-comments` has two downsides:

- it re-adjudicates settled items, and the closed-increment gate of the free-form entry is undecidable (that skill logs a line about it itself),
  so no mechanical means prevents duplicate handling;
- the genuinely new feedback is drowned in the old.

So by default only the **latest round** is taken (the default behavior of `pr_signals.py comments`). If earlier rounds really still have unhandled
entries, those are leftovers of the previous `address-review-comments`, and should be found in its triage ledger rather than by replaying
the whole comment history.

## The merge gate is "the author adjudicated", not "the reviewer agreed"

Author and reviewer are peer judges. The reviewer raises issues; the author independently judges truth and fix; **a rejection with first-hand counter-evidence
is a valid final outcome just like a fix**. So the release condition is "the author has adjudicated every point the reviewer made on the current HEAD",
not "the reviewer gave 🟢". The former is judged by `fetch`: if it opens no new round, there is no unadjudicated feedback.

🟢 cannot be the gate, for two reasons that each hold independently:

First, **it hands the veto to the reviewer**. The reviewer cannot read our replies (official docs: "Any replies you add are
visible to other people but not to Copilot"); since 2026-09-18 it marks its own findings as `Won't Fix` / `Incorrect` based only on
**subsequent commits**. The findings the author rejected, which therefore will never get a corresponding commit, by definition never go away on their own.
So once the author rejects even one, the PR can never merge — the premise of peer judges is overturned on the spot.

Measured corroboration too: on libtv-desktop PR #899, after all 18 findings were handled and Copilot itself marked them all resolved,
the overview showed `Findings: None` and not even an `Open` section existed, **yet the verdict was still `🔵 Needs a closer look`**
(the summary line still carried a no-longer-true "unresolved findings remain"). `🔵` expresses "this tree is too complex or
risky, I won't auto-approve, a human must give final confirmation", which is a different thing from "there is still unresolved feedback". With 🟢 as the release condition,
complex PRs would never merge, even with not a single finding left.

Second, **it cannot block the merge anyway**. Copilot gives a `COMMENT` review, not `Request changes`, and not a
required check. So when the review sits at 🟡 / 🔵 with all findings REJECTed, the path is a **normal squash merge**, not
`--admin`; `--admin` is reserved for the red-CI path.

What still must be guarded against is something else: **the reviewer spoke and we did not hear**. So `fetch` must take both inline comments and review
bodies — a finding written only in the body looks exactly like silence on the comments API. The verdict itself goes only into the output report:
at merge time state the verdict text, the count of unresolved findings, and each one's ledger disposition and counter-evidence.

## Adjudications must be posted back to the PR, not kept local only

`address-review-comments`' triage ledger is written only into `temp/` by default. That is a local file; machine review cannot see it,
and neither can the next person. The consequence is that **REJECT leaves no trace**: a finding rejected with good reasons comes back verbatim next round
and is rejected again — machine review has no memory, it only sees that the code is still the same. Measured (libtv-desktop): over twenty-odd rounds of one PR,
the same class of claim was raised again and again in exactly this shape.

So after every `address-review-comments` run, post `triage.md` back to the PR **verbatim** (`gh pr comment
--body-file`). It answers "why this is not changed", and that answer must live in the same place as the code.

The reader is a **human** (and the next agent), not machine review — it cannot read comments. Since 2026-09-18 its overview
keeps progress across rounds (`Open` / `Resolved since last review` / `Previously missed`), so the old claim "it has no memory"
is no longer accurate; the accurate claim is **it does not read our replies**, and only settles findings itself based on subsequent commits.

- Post verbatim. The ledger is already structured for humans; summarizing it again only creates a second version, and the two will slowly drift.
- Post APPLY too. Posting only REJECT turns the ledger into advocacy; only the full four classes show how the round was adjudicated.
- Before posting, scan for credentials and local absolute paths — the ledger is a local artifact, the PR is public.
- If it exceeds GitHub's single-comment limit, split it, do not truncate: what gets cut is always the REJECT/FLAG at the end.

## Merge method

- This repo always uses **squash and merge**.
- Touch the branch only when it is truly diverged; do not rebase proactively without divergence (`mergeStateStatus` will say).
- When diverged, **rebase by default** (`rebase-origin-main`). Only when the branch's commit count exceeds
  `--rebase-max-commits` merge `origin/master` in instead.
  Reason: rebase replays every commit, so the same conflict may be resolved once on each of several commits,
  while merge resolves it only once. And rebase's usual benefit — linear history — is worth nothing under squash:
  branch-internal history is discarded entirely on landing, and the extra merge commit never reaches master.
  `git diff <base>...HEAD` is a three-dot diff, so the PR's diff does not get dirty from a merge commit either.
- The threshold is a **proxy** for cost. What really determines cost is "how many replayed commits touch conflicting files",
  which is unknowable before starting; the commit count is the closest quantity available in advance, so it is a parameter, not hardcoded.
- Rebase force-pushes and every commit sha on the branch changes: the anchor pinned by every earlier review round
  is no longer on the branch, and all CI reruns. The merge path keeps the original shas, so old anchors stay reachable.
- All green → normal merge.
- Red but one of the three kinds of evidence holds → `--admin` merge, and state in the output **for each one** which job and which evidence released it.
  No unnamed summaries like "the rest are environment issues".

## When to stop and ask a human

- Attribution cannot be made, or the evidence only reaches "I think": stop, report the failing job and the evidence gathered.
- The failure is caused by this PR but the fix exceeds this PR's goal: stop, hand it to the developer to rule (do not widen scope along the way).
- A **major conflict** while converging divergence (both sides changed the same function, signature change, delete-vs-modify): stop.
  merge-pr runs unattended, and a major conflict is by definition the kind that needs a human call; minor conflicts and shared infra
  are handled automatically by `rebase-origin-main`'s existing rules.
- `mergeable` is not `MERGEABLE` and not caused by divergence: stop, report `mergeStateStatus`.
- The review has an unhandled FLAG: stop. By definition a FLAG needs the developer's call.

## Draft becomes ready first, no stop

On a draft PR **every workflow is skipped**: not a single check registers, and the CI wait section can only wait until timeout.
So when section 1 sees `isDraft == true`, run `gh pr ready` first and continue, instead of stopping to ask.

This is not deciding for the developer: invoking merge-pr means "merge this PR", and draft state directly contradicts it
— if it really should stay a draft, the right move is not to invoke merge-pr. Ready is also reversible (`gh pr ready --undo`), and the cost is completely
unequal to "stop once, wait for a human to come back and click, rerun everything".

## Branch first, then assert

Writing "no new round" in section 6 as a precondition assertion, with "if a new round opens, return to section 2" hanging after it, makes that branch
forever unreachable: during the CI wait a new review round can perfectly well arrive on the same HEAD; then the assertion fails first, the flow stops in section 6,
while the correct action is to go back and handle that round. Measured: libtv-desktop PR #899 had exactly this second round on the same HEAD.

## Review metrics

A single triage ledger stays in `temp/` and disappears with that run. And "is this process saving money" can only be answered by **cross-PR
trends**: how many rounds a normal PR takes to converge, how many new findings per round, the APPLY to REJECT ratio. Without a landing place it can only be judged by
impression, and impressions get skewed by a few especially stubborn PRs (libtv-desktop #899 churned six rounds, but that was because what was reviewed was the verdict parsing
itself, not representative).

The landing place is also in `temp/`: the merge happens after the PR has landed, when no branch can carry a new commit (this repo
forbids committing to master directly), and writing an **untracked** in-repo file would immediately fail the next merge-pr / gate-and-fix
precondition "the worktree must be clean". The cost is that it exists only on the machine that ran the merge.

## Why review comes before CI

How fast the two signals are is reversed across repositories, but the dependency direction is fixed: fixes from review advance HEAD,
thereby **invalidating the running CI**; while a CI conclusion never invalidates a review. So waiting on the slow one first is always waste — this repo's
CI is clearly slower than review, and waiting on CI before handling comments means running a whole lap for nothing each round.

The reverse (repositories where review is slower than CI) loses nothing either: then section 2 itself waits, CI has long finished and is sitting there, and section 3 need not
wait again. The only thing to really avoid: **waiting on the slowest signal while there are known items still to change.**

## The two signals of review coverage

`coveredAtHead` is composed from two **independent** signals: `headHasReview` (someone really reviewed the current HEAD) and
`pendingReviewers` being empty (the reviewers who re-review have caught up). Both are required — looking only at the latter, a PR whose only reviewer
"reviewed once and stopped at an old commit" would be judged covered because `recurring` is empty.

"No inline comments this round" and "the reviewer has not looked at this tree yet" look exactly the same on the comments API: zero entries.
Review events carry a commit, so it is decidable: someone reviewed this PR but has not reviewed the current HEAD means **their turn has not come yet**,
not "looked and had no comments". Without this step, merging right after a push bypasses review.

`pendingReviewers` holds only reviewers who **have proven they re-review** (reviewed >= 2 distinct commits on this PR).
Measured (libtv-desktop): Copilot re-reviews on every push, codex reviews only once when the PR opens and skips everything after. Putting "every reviewer
must cover the current HEAD" into pending never holds under that combination, and the wait inevitably runs to timeout — that is not caution, it is writing the
gate as a deadlock. Reviewers who reviewed only once appear in `reviewedOnceAt`, reported as is but not in pending.

**But this rule governs only `pendingReviewers`, not the whole wait condition.** The wait condition is `NOT coveredAtHead`,
so it still waits when `headHasReview` is false. The two signals answer different questions: pending asks "have the people who should come all arrived",
headHasReview asks "has anyone looked at this tree". **Do not** change the wait condition to "wait only when pending is nonempty":
in a PR's second round, the only reviewer has reviewed only 1 commit at that moment, does not yet count as recurring, and pending is empty;
after that change merge-pr would fall through to the section 6 Assert and STOP within seconds of a push without waiting once — and that is exactly when machine
review is about to review, and the most common round. Constructed and measured: `pendingReviewers=[]`,
`headHasReview=False`, `coveredAtHead=False` holding at the same time.

CI is also running during the wait (it starts the moment of the push), so this wait is mostly absorbed by CI on the wall clock, not wasted.

## Why only the github lane

`--github` makes `address-review-comments` use only this one lane. merge-pr runs unattended in a loop,
and local lanes sit at the HEAD each was written at; sweeping them in means re-adjudicating every round
old feedback nobody refreshed.

Fetching comments, determining rounds, adjudicating, fixing, landing and posting the ledger all belong to it. These **used to** be written in merge-pr:
calling `pr_signals.py comments` itself to fetch the latest round, rendering it into a free-form file to feed in, then posting the ledger with
`gh pr comment` itself. Not one of the three belongs to the responsibility "watch the PR until it can merge"; and the free-form
entry has no round provenance, so closed-increment gate (d) is **undecidable** on that path. As a lane, round identity is
the commit the comments pin to, and that gate becomes decidable again.

## Convergence criterion

**The convergence criterion is "nothing left to handle", not "someone reviewed".** The monitoring loop must include one
`github_review_lane.py fetch`: `coveredAtHead` only says a review event exists on the current HEAD; it knows nothing about
"whether the feedback that review left has been handled". Machine review can review another round on the same HEAD,
`coveredAtHead` does not budge, while `fetch` opens a new round.

Measured failure: monitoring watched only `coveredAtHead` and CI, reported "both signals converged", while that HEAD had
two new comments nobody had looked at. The criterion must be `fetch` returning `no-comments`, or the latest round already handled.

## CI convergence criterion

The convergence condition is "`checkCount > 0` **and** pending is empty"; both are required. Judging only pending-empty hits immediately in the tens of seconds
right after a push — at that time not a single check has registered yet, so pending is naturally empty, and "zero checks"
is taken as "all passed". Measured while dogfooding (libtv-desktop): polling right after a push, the first poll reported convergence with passed=0, and tens of seconds later
14 checks all appeared and started running. **Zero checks is not all green; it is no signal.**

## Why the entry must be a positive enumeration

Section 1b's entry admits only `CONFLICTING` and `MERGEABLE + BEHIND`; it **must not** be written as a negation like "skip when `mergeable ==
MERGEABLE` and the state is not `BEHIND`" — that is equivalent to "enter whenever `mergeable` is not `MERGEABLE`", so `UNKNOWN` also enters,
falls into the `ELSE IF ahead <= ...` branch, and rebases and force-pushes a branch that has **no conflict at all**.

`UNKNOWN` is exactly the normal state after every push — GitHub is still computing mergeability in the background; and merge-pr returns to
section 1 to retake signals every round, so this branch would be hit repeatedly. The cost is more than a wasted lap: force push rewrites every commit's
sha, every anchor pinned by earlier review rounds is invalidated, and all CI reruns.

The correct action for `UNKNOWN` is **to ask again** (section 1c): it may neither STOP (that would make merge-pr stop randomly after each push)
nor be treated as a conflict and handed to section 1b.

There are two kinds of divergence, an order of magnitude apart in cost: `BEHIND` = just behind, no conflict; `CONFLICTING` / `DIRTY` = real
conflicts for a human to resolve. `BLOCKED` / `UNSTABLE` are not divergence at all; nothing this section does will turn them green.

## The criterion that a round is done

The criterion is `triaged`, **not `reviewExists`**. The latter only says PASS 0a transcribed `review.md`, and transcription happens
**before** verification, adjudication, fixing and landing: if `address-review-comments` stops on a FLAG or crashes midway,
`reviewExists` is still true. Using it as the completion criterion would merge a round of comments never adjudicated as if handled.
`triaged.json` is written by `post-triage` **after the ledger is posted successfully**, the very last step of the whole flow.

## Review must be retaken before merging

Coverage must be asserted once more in section 6: section 2's wait may exit on timeout, and "the reviewer left no comments" and
"the reviewer did not look" look the same on the comments API; without this, a tree nobody reviewed can go all the way to merge.

**But asserting coverage alone is not enough; it must be retaken.** Section 2 handles review first and section 3 waits on CI afterwards, and this repo's CI takes
over ten minutes; in that window machine review can perfectly well review another round on the **same HEAD**. Then `coveredAtHead`
does not budge, section 2's `no-comments` is a conclusion from over ten minutes ago, and nothing would discover the new comments.

Measured: CI went all green on the 17th poll, and a new round of comments appeared on the 22nd poll — **two and a half minutes after going green**, on the same
HEAD. Running unattended to this point would merge with a whole round of comments nobody looked at.

This is the price of swapping the order of sections 2/3 (review before CI): while waiting on the slowest signal, the fastest signal can move again.
The price is worth paying, but the window must be closed in section 6.

## merge_head must be read, not hand-written

`--match-head-commit` must take the **full 40-character sha**, read with `git rev-parse`; never hand-complete it from a short sha.
Measured failure: a hand-written "full" sha had only its first 8 characters right, and GitHub replied
`Head branch was modified. Review and try the merge again.` — which sounds like someone pushed to the branch,
so the investigation ran off course into branch history, while the branch had not changed by a single byte.

## Holding before merge

`--hold-before-merge` is for a developer manual test of the exact tree that will merge. It runs sections 1-6 in full, including the
review-lane retake and every merge assertion, and stops only where the next step would be `gh pr merge`. `--no-merge` stops at the
start of section 6 and proves less: it skips the retake and the assertions, so it cannot say the PR is ready to merge.

The hold report states `ready to merge at <merge_head>, holding for developer verification`, then the OUTPUT items as of that moment:
per-check CI conclusions with each red check's attribution and evidence, review handling and the verdict text, the merge method that
would be used (normal, or `--admin` with the release basis for each remaining red check), and any unhandled FLAG last. There is no
merge commit yet, and no review-metrics line is appended.

After the developer approves, run merge-pr again **without** the flag. It re-verifies from section 1, because HEAD, CI or review may
have moved while the developer tested, and merges only if every assertion still holds; `--match-head-commit` pins the merge to the
re-verified head. A push during the test therefore leads to a fresh full pass, not a merge of the tree the developer did not test.
