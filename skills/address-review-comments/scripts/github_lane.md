# GitHub lane resolution

Execute this procedure only from PASS 0a of the [entrypoint](../SKILL.md),
with its INPUT and HARD RULES. Return to that entrypoint for sanitization
unless this procedure returns normally or STOPs.

```text
The GitHub machine reviewer is one reviewer lane of the PR; its output just lives remotely instead of in `temp/`. Fetch it and
transcribe it into a `review.md` like everyone else's, and from then on **it is no longer special**: the lane discovery glob already scans it,
and deduplication, evidence gathering, and adjudication all follow the same path.

`<pr>` = the number given by `--pr`; if not given, take the current branch's PR via `gh pr view --json number`; if that fails, STOP.
-- Both commands in this section need the PR number, which previously did not exist in INPUT at all, so the flow had no way to obtain it here.
--   The caller (merge-pr) has already resolved the number itself; passing it explicitly is more reliable than letting this skill guess again.

Run:
  python3 skills/address-review-comments/scripts/github_review_lane.py fetch --pr <pr>

`status` has four values: `head-moved` (race, fewest fields), `no-comments`, `created`, `reused`.
Only the last two provide `roundDir`, `source` (verbatim source dump + per-item timestamps), `baseRef` / `baseSha`,
and `reviewExists`; the first two lack these fields and each has its own branch below; **never let them fall into the later branches**.
-- Round identity is judged by **content** (id set + `updated_at` + body digest), and location uses `original_line`;
--   the reasons for both and the pitfalls hit are in [guide.md](../guide.md) "Round identity is judged by content" and "Location uses original_line".

**One round has two lanes: inline comments (`commentCount`) and review bodies (`reviewCount`).** A body whose verdict is not 🟢
is always an outstanding comment and forms a round on its own; see [guide.md](../guide.md) "The review body is itself a lane of comments".

IF `status == head-moved`:
  STOP. Someone pushed between fetching HEAD and fetching comments, so this snapshot is a race artifact: `fetch` explicitly **does not return**
  `roundIsCurrentHead`, `roundDir`, `source`, or `reviewExists`. Discard it and fetch again; do not infer a round.
  -- Why this must fail closed: [guide.md](../guide.md) "head-moved must fail closed".

IF `status == no-comments`:
  This lane has no output: inline comments and outstanding review bodies are **both** zero. **IF `--github`: there is nothing to address in this run; return normally**
  (0 claims, no triage written, no ledger posted) and let the caller continue; never proceed to sanitize, where the lack of input would STOP.
  ELSE: skip this section and use the local lanes as usual.
  -- "The review is clean" is the most common case. Turning it into a STOP would halt merge-pr exactly when it should continue.

IF `roundIsCurrentHead == false` AND `cleanReviewAtHead == false`:
  STOP. Commits were pushed after the review ran: **no review has seen the tree about to be merged**.
  -- This must stop here and cannot wait for the caller to check afterwards: transcription, adjudication, and landing all happen inside this skill,
  --   and by the time merge-pr gets the return value, a stale review may already have been landed by commit-and-push.

IF `cleanReviewAtHead == true`:
  The current HEAD has a re-review **with no inline comments and a clean verdict**: the reviewer looked at this tree and had no comments.
  Inline comment groups sitting on old anchors is normal, not "nobody looked".
  -- No need to judge the verdict yourself: 🟡 / 🔵 / unrecognized bodies would already occupy the current HEAD, and this branch would be unreachable.
  -- It looks the same in `rounds` as "commits were pushed after the review ran"; the difference and consequences are in
  --   [guide.md](../guide.md) "A clean re-review is not 'nobody looked'".
  IF the latest round has `triaged == true`: nothing to address in this run; handle it the same as `no-comments`.
  ELSE: address that round as usual (it is an unfinished set of comments on an old anchor).

IF `reviewExists == false`:
  Read `<roundDir>/source.md` and write `<roundDir>/review.md` per
  [github-lane-transcription](../references/github-lane-transcription.md).
  That file specifies the header field values (`**分支**` uses the anchor, not head; `**判定**` is always Needs Refinement; etc.),
  the finding structure, and the fixed prior-round closure wording.
  -- **This is the agent's job, not the script's job**: you have to understand what each comment says, which dimension it falls in, and where the evidence points
  --   before you can write a finding; a script can only move text around.

IF `--github`: use **only** this lane in this run, skip legacy local lane discovery, and pass
`<roundDir>/review.md` as the **sole** `--review-file` to the sanitize step below.
-- It cannot remain a placeholder: `--github` skips legacy discovery, which is the only place that would fill in a concrete path;
--   without binding it, sanitize gets no input file and exits, and the GitHub lane never reaches PASS 1.
ELSE: this lane enters the discovery below together with the local lanes (it lives under `temp/review-pr/<slug>/github/`,
which the glob already scans).
```
