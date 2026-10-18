# GitHub lane transcription specification

The strict structure for writing `<roundDir>/source.md` (a verbatim dump of the GitHub comment
source) into `<roundDir>/review.md`. The overall template follows the PR template in
[review-format](../../../docs/guides/review-format.md); this file only specifies the GitHub lane's
**values and constraints** relative to that template.

**This is the agent's job, not the script's job**: you have to understand what each comment says,
which dimension it falls in, and where the evidence points before you can write a finding; a
script can only move text around.

## Header fields

| Field | Value | Constraint |
|------|------|------|
| `**审查者**` | the lane name, i.e. `github` | |
| `**分支**` | `<branch> @ <anchor short sha>` | **It is `anchor`, not `head`** |
| `**基线**` | `baseRef @ baseSha` | Use the values `fetch` already resolved; do not run another `rev-parse` |
| `**锁定目标**` | verbatim text of `temp/review-pr/<slug>/.locked-goal` | STOP if it cannot be obtained |
| `**判定**` | always `Needs Refinement` | See "This lane never gives Ready" below |

Writing `head` instead of `anchor` in `**分支**` lets the lane falsely claim this review round
covered the current tree, and the closed incremental gate (d) relies on exactly this provenance.

`**锁定目标**` is the authority for the GOAL & SCOPE GATE and **must not** be guessed from the
diff, commits, or PR description: a guessed goal puts every later scope decision on a premise that
nobody confirmed.

## finding

Each comment is transcribed into one finding: location, dimension, observation, evidence, impact.
The source text is **not rewritten and not summarized**, and the evidence states
`GitHub inline comment #<id>` so it can be traced back to its origin.

The "review body" section in `source.md` also produces findings, with the origin written as
`GitHub review #<id>`:

- **Each** item in the body's `Suppressed comments (N)` / `Previously missed (N)` sections produces
  its own finding, located at the `path:line` that item gives. They are the same kind of thing as
  inline comments, just posted somewhere else; missing them means this round was effectively never
  addressed: none of those comments appear among the inline comments.
- When the body has only a verdict line and no concrete comments (for example `🔵 Needs a closer
  look` saying "too complex or risky, needs a human to confirm"), it still produces **one** finding
  whose observation quotes the verdict line and reason verbatim. Whether it needs a developer
  decision is adjudicated in PASS 3; the transcription stage does not prejudge it. Its only job
  here is to get that sentence into the set of items to address instead of silently swallowing it.
- When a review has **only a state, with no body and no inline comments** (a human only clicked
  Request changes), it likewise produces **one** finding: the location is that review's
  `state: CHANGES_REQUESTED`, and the observation is "the review requested changes but left no
  body". Without it, `review.md` would have 0 claims while PASS 1 still marks the round as
  addressed, and a review that explicitly requested changes would be merged right past.

## Prior-round closure

- Round 1: the body must be exactly `无。`
- Round 2+: the fixed empty table + the fixed empty summary

The machine reviewer **can re-raise a previously rejected comment at any time**: it has no memory
and only sees that the code did not change. So each of its rounds is a complete opinion on the
current tree: prior rounds do not constitute items to close, and re-raised ones appear in this
round as new findings.

## This lane never gives Ready

The machine reviewer does not make the "ready to merge" judgment; it either raises issues or does
not, so `**判定**` is always `Needs Refinement`. The PASS 0 rule "a lane whose verdict is Ready and
whose sync list is empty does not enter this run" never applies to it.
