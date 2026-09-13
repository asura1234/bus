---
name: review-pr
description: Review the committed Bus diff against an explicit base for correctness, safety, regressions, scope, and test adequacy. Use when asked to review, re-review, or audit a PR or branch.
---

# Review PR

Read [guide.md](guide.md) before reviewing.

Input accepts `--base <ref>`, `--reviewer <lane>`, and optional
`--plan <path>`. Default base is `origin/main` only when it resolves. Never infer
the base from remote HEAD. Preserve the same reviewer lane in follow-up rounds.

## Boundary

- Review the committed `<base>...HEAD` diff. Exclude uncommitted user changes
  from findings unless they prevent reliable review.
- Do not modify production code, commit, push, rebase, or open/update a PR.
- A reviewer may add narrowly scoped regression tests to prove a suspected
  runtime bug, but only with clear ownership and only in test files. Otherwise
  remain read-only.
- Artifacts belong under `temp/review-pr/<branch>/<reviewer>/`.

## Round 1

1. Fetch the selected base remote when network access is available, resolve the
   immutable base SHA, and record HEAD.
2. Read the full diff, changed-file list, commit list, related plan, and relevant
   callers/tests. Classify unrelated or oversized change separately.
3. Review reachable behavior for correctness, error handling, state lifecycle,
   concurrency, security/trust boundaries, portability, performance, and API or
   persistence compatibility.
4. Check tests against the actual failure modes. A test that only asserts its
   mock or duplicates implementation logic is not proof.
5. For a suspected behavior bug, prefer a focused test that fails on reviewed
   HEAD and would pass after the required fix. Run only the narrow test first.
6. Report material, actionable findings with priority, exact location, failure
   path, evidence, and impact. Do not report general cleanup or style opinions.

## Follow-up rounds

Read the prior review and author disposition. Verify only earlier findings, the
new committed delta, and its direct consequences. A finding fixed at current
HEAD is resolved even if an older comment was originally valid.

## Verdict

- `Ready`: no material finding remains.
- `Not Ready`: one or more material findings remain.

Write and return `review.md`. Mention any reviewer tests created and leave them
uncommitted for the author unless the user explicitly requests otherwise.
