# GitHub review lane judgment principles

This file holds only the judgment specific to the **GitHub lane (PASS 0a)**. Cross-lane general
principles such as disposition classification, disposition mapping, and evidence boundaries remain
in the [Review Response Guide](../../docs/guides/review-response-guide.md) and are not repeated
here.

The GitHub machine reviewer is one reviewer lane of the PR; its output just lives remotely instead
of in `temp/`. Fetch it and transcribe it into a `review.md` like everyone else's, and from then on
**it is no longer special**: the lane discovery glob already scans it, and deduplication, evidence
gathering, and adjudication all follow the same path. Everything below is required for "no longer
special" to actually hold.

## Round identity is judged by content, not by anchor

The machine reviewer can rerun on **the same commit**; then the anchor is unchanged but the
comments are new. Comparing only the anchor treats the new round as "already fetched", and that
round is never addressed. So round identity includes the id set, `updated_at`, and a body digest;
comparing only ids misses "a comment was edited": the id stays the same while the body changes.

A new round renders only **the delta relative to rounds already on disk**: when the reviewer
reruns on the same commit, old and new comments merge into the same anchor group, and rendering the
whole group would re-address and re-post every previously adjudicated item, the exact opposite of
"address only the latest round".

## Location uses original_line, not line

`line` is re-resolved by GitHub relative to **the current HEAD** and drifts with every push;
`original_line` is the line number in the tree where the comment was written, paired with
`original_commit_id` (this round's anchor), and never changes.

If the digest followed the drifting `line`, a round that was **already adjudicated and replied to**
would be reopened as a new round, and `address-review-comments` would address the same comments
again and post the ledger again. Observed: after one push, the `line` of 2 comments on a PR drifted
from 44/212 to 47/223 while the id, body, and `updated_at` did not change at all, yet a whole
duplicate round appeared. The location must also come from the same tree as the anchor; otherwise
the heading line of `source.md` points to a location that does not exist at the anchor.

## head-moved must fail closed

When someone pushes between fetching HEAD and fetching comments, `fetch` returns `head-moved` and
explicitly **does not return** `roundIsCurrentHead`, `roundDir`, `source`, or `reviewExists`. The
flow must stop right there; otherwise it falls into a later branch and reads a `roundDir` that does
not exist. The correct handling is to discard this racy snapshot and fetch again, not to infer a
round from `head` / `previousHead`.

## A clean re-review is not "nobody looked"

The reviewer can give a **clean re-review with no inline comments** on the current HEAD. The inline
comment groups then still sit on old anchors, which looks exactly like "commits were pushed after
the review ran" in `rounds`: both appear as "the anchor of the latest comment group is not HEAD".

Looking only at `rounds` judges a clean re-review as nobody having looked and STOPs, so a clean
re-review can never move merge-pr forward, even though that is precisely the reviewer saying "this
is fine". `cleanReviewAtHead` fills in the missing half from review events: there is no inline
comment group on the current HEAD, but there is a review event.

## The review body is itself a lane of comments

One review produces two outputs: inline comments and the review **body**. The body is not a cover
sheet for the comments: the machine reviewer was observed writing concrete comments
in full inside the body's `Suppressed comments (N)` / `Previously missed (N)` sections while the
body itself says `Comments generated: 0`. Reading only inline comments, such a round has zero
comments and looks exactly like "looked and had no comments" on the comments API, so it is waved
through as a clean re-review, even though it is the only place carrying that comment.

The body carries a verdict line. Three were observed: `🟢 Approval recommended`,
`🟡 Changes recommended`, `🔵 Needs a closer look`. **Only 🟢 / `Approval recommended` counts as
clean**; the other two both mean "this tree still has something to say": 🟡 points at concrete
comments, and 🔵 is the reviewer explicitly declining to approve and asking a human to take another
look. Merging those as ready treats the reviewer's spoken reservations as silence.

**The verdict line is not necessarily the first heading.** Observed: a body in the
`ccr-overview-v2` layout first opens a `## Copilot review overview`, with the verdict written
beneath it as `### 🟡 Changes recommended`. Taking only the first heading reads the verdict of every
v2 body as `Copilot review overview`; fail-closed then covers only half: non-approval verdicts are
indeed still outstanding, but **clean re-reviews can no longer be recognized either**, so every
"looked and had no comments" spuriously opens a round. The criterion therefore scans all headings,
and the body is clean only if any one of them matches a clean verdict.

A clean verdict must match the **whole line**. Written as "contains some clean word",
`Disapproval recommended`, `Not approval recommended`, and `🟢 Changes recommended` would all be
judged clean: anything that happens to carry a clean word gets through, and fail-closed becomes
decoration.

An unrecognized verdict line is always treated as outstanding. These markers are owned by an
external bot that can change the wording or add new ones at any time: treating unknown as clean
**silently skips an entire review round**, while treating unknown as outstanding at worst
adjudicates one extra time and leaves a trace in the ledger. The direction is chosen by
consequence, for the same reason as "GitHub token prefixes must be listed in full".

## The delta is judged by content, not by id

The round delta used to be subtracted by id: an id in the addressed set was skipped. That made
"this item was edited" look exactly like "this item is unchanged", so rewritten comments never
entered any round. The fallback "if both lanes are empty, fall back to the whole group" fired only
when there were no new items at all; as soon as a new comment arrived at the same time it no longer
covered the case.

And that fallback itself did harm in the opposite direction: when a review body is withdrawn the
digest also changes, but that is **something removed**, not something added, and falling back to
the whole group re-addresses comments that were already adjudicated and already posted in the
ledger.

So `source.json` records per-item content digests (`itemDigests`) and the delta is judged by
content; when neither lane has anything new, the already-addressed round is reused instead of
opening a new one. Old rounds without `itemDigests` are still recognized by id; otherwise, the
moment the upgrade lands, every addressed item of every round on disk would be treated as new
because no digest could be found for it.

There is only one exception, in one direction: when the body has comment sections, it counts as
outstanding even if the verdict line says approval; those comments cannot be read anywhere else.
Conversely, an empty body (just the envelope for inline comments) and `DISMISSED` (withdrawn) do not
form a round.

## Known boundary: an edited body on an old anchor is not addressed

The REST review object has only `submitted_at` and **no** `updated_at`: when the body is edited,
that value does not change at all. So in the combination "a body on an old anchor is rewritten
while a newer anchor also has outstanding content", round selection cannot see that the old group
changed, and that rewrite never enters any round. Inline comments do not have this problem: their
`updated_at` changes with the edit, so that group is selected.

This is a **deliberately kept boundary**, not a defect waiting to be fixed: round selection runs on
"address only the latest round" (for the reason see the same-named section of the merge-pr guide),
and content on old anchors by definition does not belong to this pipeline; it also does not block
merging: merge-pr's gate is "the review on the current HEAD has been addressed". Observed: the
machine reviewer edits its own overview only within the same anchor; cross-anchor edits have not
occurred.

Changing it would mean switching round selection from "the anchor with the latest activity" to
"the latest **unaddressed** anchor", which would also reopen old rounds that were **never
addressed** (for example ones that stopped midway on a FLAG), which merge-pr explicitly does not
want to replay. That is a trade-off at the round-model level, not something to change in passing
at the signal-fetching layer.

## This lane never gives Ready

The machine reviewer does not make the "ready to merge" judgment; it either raises issues or does
not, and `🔵 Needs a closer look` is precisely it saying "this judgment is not mine to make".
Therefore the transcribed `**判定**` is always `Needs Refinement`. The rule "a lane whose verdict
is Ready and whose sync list is empty does not enter this run" never applies to it.

It also **can re-raise a previously rejected comment at any time**: the official documentation
states that a re-review may repeat earlier comments, even if you have already resolved or
downvoted them. So each of its rounds is a complete opinion on the current tree: prior rounds do
not constitute items to close, and re-raised ones appear in this round as new findings.

**Replying to its thread lets it receive the decision; posting a separate comment does not.** The
GitHub documentation page still says "Any replies you add are visible to other people but not to
Copilot", but that is outdated: the 2026-09-18 changelog says it honors replies, and observation
agrees: on a LibTV PR (#899), after replying to 18 threads one by one, **all** of them were marked
resolved, including one with **no corresponding fix commit** and `outdated == false` (the one the
author rejected). So rejections are received, and the channel is the thread reply.

Conversely, posting a separate PR comment (the ledger) does not work: that is an issue comment, not
on any review thread, and the reviewer does not read it; observed: the ledger received no reactions
at all. `@`-mentioning it does not help either: `@copilot` reaches the coding agent (the docs say
it pushes commits to the PR branch), not the code reviewer. The ledger is still posted, but that is
for humans and for the audit trail.

## The completion marker is written in the last step

`review.md` already exists after the PASS 0a transcription, and transcription happens **before**
verification, adjudication, remediation, and landing; stopping midway on a FLAG or crashing leaves
it behind. Therefore `reviewExists` cannot serve as the completion criterion; otherwise a round of
comments that was never adjudicated would be treated by the caller as addressed and merged
directly.

`triaged.json` is written by `post-triage --round-dir` **after the ledger is posted successfully**,
which is the last step of the whole flow. A round whose ledger could not be posted (credential hit,
too long, gh failure) did not finish and must not leave behind a file saying it did.

## Lane directories are created by local branch

The artifact is for lane discovery, and that side (`review_round.py`) creates directories by
`git branch --show-current`. With a fork checkout or a locally renamed branch, that diverges from
the PR's `headRefName`, and writing into a `headRefName` directory means writing where nobody will
glob. A detached HEAD errors out directly and **does not fall back** to `headRefName`; that kind of
fallback is silent.

The lane name itself is also joined straight into the path, so it must be a single safe segment;
anything out of bounds is rejected rather than sanitized: sanitizing would make the name passed in
and the name written to disk not the same.

## Scan the ledger before posting it

The ledger is a local artifact under `temp/`, and the PR is public. Before posting, scan for
credentials and local absolute paths; on a hit, STOP and let a human decide how to redact it:
**do not rewrite the content automatically here**, because rewriting makes what is posted differ
from the ledger.

GitHub token prefixes must be listed in full (`ghp_` / `gho_` / `ghu_` / `ghs_` / `ghr_` /
`github_pat_`): prefixes are an enumeration, and missing one lets that whole class through. Absolute
paths are blocked **as a whole class**, not by prefix enumeration: enumerating `/Users` `/home`
inevitably misses `/tmp`, `/private/var`, `/opt`, `/Volumes`, and the paths that appear most often
in ledgers are exactly pytest tmp paths like `/private/var/folders/...`. `~/…` needs its own
entry: the negative lookbehind of the absolute-path rule explicitly forbids `~`, so home-relative
paths never match it, yet they leak the local directory structure just the same.


## Per-lane unique claim count

A lane whose unique count stays at 0 over time has every item it raises also raised by someone
else, so running it alongside is just burning tokens repeatedly; this is the only basis for judging
"whether a given reviewer lane is still worth running together".

Two definitions must not be blurred: count by **deduplicated canonical claims** after PASS 1, not
by raw assertions (the same comment written as three paragraphs in one lane does not mean that lane
saw three more problems); it is **unrelated** to disposition, and a REJECTed claim still counts as
raised by that lane (it measures whether perspectives overlap, not whether they were right; mixing
the two would count a unique perspective that was right only once as worthless).
