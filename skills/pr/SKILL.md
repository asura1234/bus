---
name: pr
description: Feature-branch PR workflow. Land uncommitted work first and rebase onto origin/master, immediately create or refresh an English Draft PR, then delegate to gate-and-fix to converge code gates, synchronize documentation, and refresh the final body. Dead-code cleanup does not run by default; only an explicit --delete-dead-code delegates to delete-dead-code before the gates. Use when the user asks to "open a PR", "create a PR", "submit a PR", "update a PR", or "generate a PR description".
---

# PR

```text
INPUT $ARGUMENTS = [--plan <plan-file>]... [--delete-dead-code]
plans = every plan path the developer supplied this time, in order of appearance; never guess from diff, commits, or historical state.

-- Rules
- Code narrative takes only the current repository, current feature branch, and actual changes as its source of truth; plans lock only goal and non-goals.
- Prior review-plan or review-pr invocation is not required.
- Do not read or guess another skill's private control state, callbacks, or temporary artifacts; the only cross-skill
  handoff is `temp/review-pr/<branch>/.locked-goal` and `temp/review-pr/<branch>/.locked-non-goals`, written and returned by `pr_goal_context.py`.
- PR title and narrative are English; fixed headings and machine tokens stay byte-compatible with the format SOT
  `references/pr-template.md`. The initial Draft after rebase and the final body must pass `pr_format_check.py --phase draft|final` respectively; never skip either.
- Every commit and push must be delegated to `commit-and-push`; every rebase must be delegated to `rebase-origin-main`; never duplicate their Git protocols.

========== PREFLIGHT ==========

repo = `git rev-parse --show-toplevel`
branch = `git branch --show-current`

IF repo missing
  ERROR "The current directory is not a Git repository."
IF branch empty OR branch IN {main, master}
  ERROR "A PR cannot be created from detached HEAD or main/master."

Run:
  git status --short
  git fetch origin
Assert `origin/master` resolves.

working_changed = tracked + untracked files in the current worktree
committed_changed = `git diff --name-only origin/master...HEAD`
ahead = `git rev-list --count origin/master..HEAD`

IF working_changed empty AND committed_changed empty AND ahead == 0
  ERROR "The current branch has no work to commit or publish relative to origin/master."

Inspect the complete current diff, untracked files, and commits. Confirm they describe one coherent
PR purpose. Do not silently discard, stash, rewrite, or absorb unrelated developer changes.

========== LAND UNCOMMITTED WORK (ONLY IF DIRTY) ==========

IF the worktree has uncommitted changes belonging to this PR
  Invoke `commit-and-push`.
  It must preserve unrelated dirty files, split commits by single purpose, and push with the
  explicit same-name refspec required by the repository Git safety rules.

  Assert:
    - every intended PR file is committed;
    - unrelated pre-existing dirty files remain untouched and uncommitted;
    - remote branch contains the local HEAD.

ELSE
  Skip this stage entirely and go straight to the rebase. A clean worktree needs no commit, and
  invoking `commit-and-push` just to have it report "nothing to do" is noise.

This stage exists only to make the next one possible: `rebase-origin-main` requires a clean
worktree. Unrelated pre-existing dirty files that are NOT part of this PR stay uncommitted — if
they block the rebase, that is a blocking report, never a reason to absorb them into the PR.

`gate-and-fix` later requires the same clean worktree to prove its artifact against one committed
tree. Therefore, if unrelated dirty files remain after this stage, STOP before readiness convergence
and report them as excluded; do not run gates against a tree the artifact cannot identify.

========== REBASE ==========

Invoke `rebase-origin-main` with the Bus adapter base `origin/master`.
It owns conflict triage, dependency/submodule resync when applicable, and force-with-lease push; this skill no longer runs rebase itself.
If it reports a non-empty `dropped_shared_infra`, it must be written verbatim into the PR description's verification/risk notes; never omit it.

Rebase MUST happen before the readiness gates, not after. Gates run against the merged result, so a
gate that passed on the pre-rebase tree proves nothing about what actually lands — and re-running
every gate after a late rebase doubles the most expensive part of this workflow (the Cargo, Python, and Bun gates). Never describe or publish a pre-rebase tree.

baseline = `git rev-parse origin/master`; draft_head = `git rev-parse HEAD`.
Assert the remote branch tip is exactly `draft_head`.
draft_diff = `git diff <baseline>...<draft_head>`; draft_commits = `git log <baseline>..<draft_head> --oneline`.

========== LOCK PR INTENT ==========

IF plans nonempty:
  Run `python3 skills/pr/scripts/pr_goal_context.py --branch "<branch>" --output
  "<ignored-temp-goal-context>"`, appending `--plan "<path>"` once per plan in original order.
ELSE:
  Write a single goal from draft_diff / draft_commits; write only evidence-backed intentional exclusions as non-goals, otherwise write `无`.
  Write the two into ignored temp goal/non-goal files respectively, then Run:
    python3 skills/pr/scripts/pr_goal_context.py --branch "<branch>" \
      --output "<ignored-temp-goal-context>" --goal-file "<goal-file>" \
      --non-goal-file "<non-goal-file>"
IF command fails: STOP; never hand-parse plans or create the lock yourself.
Capture `GOAL_CONTEXT_FILE`, `LOCKED_GOAL_FILE`, and `LOCKED_NON_GOALS_FILE`; Read(GOAL_CONTEXT_FILE) completely.
The two sections of that context, `.locked-goal`, and `.locked-non-goals` are locked by the script and must not be rewritten afterwards.

========== PUBLISH DRAFT OR CAPTURE EXISTING READY PR ==========

Read(references/pr-template.md)
Derive the initial English title and narrative only from immutable draft_diff / draft_commits; goal and
non-goals come only from GOAL_CONTEXT_FILE. Fill every template section. `文档同步` uses only `- [ ]` pending
items; `自测 / Agent 测` retains the pending readiness gate; `其他说明` states that this is the
post-rebase snapshot and selected cleanup/gates/docs are still pending. Never mark unperformed work as passed.

Write `draft_body_file` under ignored `temp/`, then run:

  python3 skills/pr/scripts/pr_format_check.py --phase draft \
    --template skills/pr/references/pr-template.md --title "<draft-title>" \
    --body-file "<draft-body-file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF the check fails: fix and rerun; do not publish an unvalidated Draft.

existing = `gh pr list --head <branch> --state open --json number,title,url,isDraft`
STOP if it contains more than one PR. `created_as_draft = false`.
IF existing Draft PR found:
  `gh pr edit <number> --title "<draft-title>" --body-file <draft-body-file>`.
ELSE IF existing ready PR found:
  Capture it but leave its title, body, and ready status unchanged until finalization.
ELSE:
  `gh pr create --draft --head <branch> --base master --title "<draft-title>" --body-file <draft-body-file>`;
  `created_as_draft = true`.

Capture PR number/URL. A PR created here stays draft; never run `gh pr ready`. Draft creation/editing
happens before dead-code cleanup, gate-and-fix, or update-docs; an existing ready PR is only captured
here and remains unchanged until finalization. If a later stage blocks, preserve the PR's current
draft/ready state and report its URL plus the blocker; an early Draft keeps its pending body.
The explicit PR request authorizes only this create/edit and the later final edit; never approve,
merge, close, or delete anything without separate authorization.

========== DEAD CODE (opt-in) ==========

IF `--delete-dead-code` was not passed:
  Skip; carry that fact into final `## 其他说明`.
ELSE:
  Invoke `delete-dead-code --base <baseline>`. It owns scope derivation, fan-out, scope check, and
  landing via `commit-and-push`; carry its `LIKELY` / `KEPT` items into the final PR body.
  IF blocker or out-of-scope change: STOP and report; never gate an unverified deletion tree.

========== READINESS CONVERGENCE ==========

Invoke `gate-and-fix --base <baseline>`. It owns the complete concurrent gate/remediation/
commit-and-push loop and the ban on E2E. Capture its final PASS artifact and Head as `gated_head`.
`origin/master` moving after `baseline` does not trigger another rebase.

copy_diff = `git diff <baseline>...<gated_head>`; copy_commits = `git log <baseline>..<gated_head> --oneline`.

========== FINALIZE ==========

Invoke `update-docs --base <baseline>`. It is the sole tracked-worktree writer, performs no Git or
GitHub mutation, and must stop on any overlapping active owner.

Wait for it. Preserve its audit path and run `git diff --check`. If it fails on the docs delta,
return the exact failures to that owner; it fixes only owned files, treats the old audit as invalid,
and reruns `update-docs` to produce a replacement finalized audit. Repeat until `git diff --check`
passes. Assert every worktree change is an audit target or required agent-instruction symlink; any other
delta is a blocker. Capture its normalized path/status set as `docs_delta`.

Then write the final English title and body yourself, under one hard constraint:

**The narrative may come only from the immutable `copy_diff` / `copy_commits` captured above; goal and non-goals come only from
GOAL_CONTEXT_FILE.** Never re-derive from the live worktree, live `HEAD`, or the mutable `origin/master` —
both the docs commit and `origin/master` may have moved by now, and deriving from them writes things that do not belong to this PR into the body.
Exclude the `update-docs`-only delta and commit from the title, size, summary, and change list.

-- No separate read-only PR finalizer subagent is dispatched here anymore. It was introduced to "shorten the serial wait between
--   the documentation audit and description generation" (libtv-app #275), but the body's `## 文档同步` must wait for the
--   `update-docs` audit before it can be filled; the two stages are chained by a data dependency, so the parallelism existed only
--   in form. In exchange it brought a handoff protocol of "stay available / two sends / never read live HEAD" with its own failure
--   mode (once the finalizer is gone, the whole flow stalls at the last step).
--   Same criterion as libtv-app #383: fan-out is only worth paying for when it brings a genuinely independent perspective (the
--   review side) or genuinely disjoint work (gate remediation); here it is neither. The bold constraint above was the entire
--   reason that subagent existed; writing it down directly is enough, and no process boundary is needed to enforce it.

Fill every template section:

- Insert the full `## 目标` and `## 非目标` sections of GOAL_CONTEXT_FILE verbatim; never add source labels,
  summaries, translations, or separators. Same-named sections from multiple plans were already appended by the renderer in input order.
- Populate `## 文档同步` by running the following against the latest audit returned by
  `update-docs`, then insert stdout verbatim (do not hand-edit, translate, reorder, or infer):

    python3 skills/update-docs/scripts/docs_audit.py render-pr --audit "<audit>"

- Populate `## 自测 / Agent 测` only with the final PASS gate-and-fix artifact **and its Head**,
  the finalized documentation audit plus `git diff --check`, and other verification actually
  executed in this run, all checked `- [x]`. The gate artifact proves only `gated_head`; the audit
  and `git diff --check` separately prove the not-yet-landed documentation delta. Never claim E2E,
  review, or manual verification that was not actually completed — unperformed verification goes to
  `## 其他说明` as pending.
- When the diff touches delivery, callbacks or launch, run `just e2e` before merging (live model usage,
  so gates never run it); record its table under `## 自测 / Agent 测`, or list it as pending in `## 其他说明`.

Write `final_body_file` to temp, then gate it:

  python3 skills/pr/scripts/pr_format_check.py --phase final \
    --template skills/pr/references/pr-template.md --title "<final-title>" \
    --body-file "<final-body-file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF the check fails: fix and rerun; do not replace the Draft with an invalid final body.
Assert remote branch tip is `gated_head`, then `gh pr edit <number> --title "<final-title>"
--body-file <final-body-file>` without changing draft status.

========== LAND FINAL DOCUMENTATION ==========

IF update-docs changed documentation
  invoke `commit-and-push` once.

final_head   = `git rev-parse HEAD`
final_status = `git status --short`
branch       = `git branch --show-current`

Assert the worktree is clean, the remote branch contains `final_head`, and any
`gated_head..final_head` path/status set equals `docs_delta`. Do not regenerate or reconcile the PR
narrative from this docs-only commit.

Then read back and verify the published state — `gh pr edit` can silently fail or truncate,
and this is the only check that proves what is actually on GitHub:
  published = `gh pr view <number> --json title,body,headRefOid,state,baseRefName,isDraft`
Bind returned fields as `published_title`, `published_body`, `published_head`, `published_state`,
`published_base`, and `published_is_draft`; write `published_body` byte-exact to ignored `temp/` as
`published_body_file`.
  python3 skills/pr/scripts/pr_format_check.py --phase final \
    --template skills/pr/references/pr-template.md --title "<published_title>" \
    --body-file "<published_body_file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF `published_title` or `published_body` differs from the intended title/body or fails the check
  Fix via `gh pr edit` and re-verify; do not report success with a non-conforming PR.
Require `published_head == final_head`, `published_state == OPEN`, `published_base == master`, and, for a
PR created this run, `published_is_draft == true`; otherwise STOP with the observed fields.

========== RETURN ==========

Report:
  - PR title and URL;
  - PR publication status: for an existing Draft or newly created Draft, note it was refreshed/opened
    immediately after rebase and, if created here, stays draft until the developer marks it ready after
    `review-pr`; for an existing ready PR, note it was captured after rebase and refreshed only during
    finalization;
  - final HEAD;
  - commits created or updated;
  - final PASS gate-and-fix artifact (with Base / Head), the final documentation audit, and `git diff --check` evidence;
  - `GOAL_CONTEXT_FILE`, plus `LOCKED_GOAL_FILE` and `LOCKED_NON_GOALS_FILE` for `review-pr` to consume;
  - pr_format_check result on the published PR;
  - any manual verification still pending;
  - unrelated dirty files explicitly excluded from the PR.
```
