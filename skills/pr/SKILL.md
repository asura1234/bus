---
name: pr
description: Feature-branch PR workflow. Land uncommitted work, rebase onto origin/master, then create or refresh an English Draft PR and its body. It does not run gates, documentation audits, or dead-code cleanup; the orchestrator sequences gate-and-fix, update-docs, and delete-dead-code itself and may hand their results to pr as optional --verification / --docs-audit inputs. Use when the user asks to "open a PR", "create a PR", "submit a PR", "update a PR", or "generate a PR description".
---

# PR

```text
INPUT $ARGUMENTS = [--plan <plan-file>]... [--verification <file>] [--docs-audit <audit>]
plans = every plan path the developer supplied this time, in order of appearance; never guess from diff, commits, or historical state.
verification = optional Markdown file of `- [x] ` lines, each naming verification actually executed and the commit it proves
  (for example a gate-and-fix PASS artifact with its Head); supplied by the caller, never produced here.
docs_audit = optional finalized `update-docs` audit path; supplied by the caller, never produced here.

-- Rules
- Code narrative takes only the current repository, current feature branch, and actual changes as its source of truth; plans lock only goal and non-goals.
- Prior review-plan or review-pr invocation is not required.
- This skill never runs or requires `gate-and-fix`, `update-docs`, or `delete-dead-code`, or their artifacts. The orchestrator
  sequences them; their results reach the body only through `--verification` / `--docs-audit`.
- Do not read or guess another skill's private control state, callbacks, or temporary artifacts; the only cross-skill
  handoff is `temp/review-pr/<branch>/.locked-goal` and `temp/review-pr/<branch>/.locked-non-goals`, written and returned by `pr_goal_context.py`.
- PR title and narrative are English; fixed headings and machine tokens stay byte-compatible with the format SOT
  `references/pr-template.md`. Every published body must pass `pr_format_check.py`; never skip it.
- Every commit and push must be delegated to `commit-and-push`; every rebase must be delegated to `rebase-origin-main`; never duplicate their Git protocols.

========== PREFLIGHT ==========

repo = `git rev-parse --show-toplevel`
branch = `git branch --show-current`

IF repo missing
  ERROR "The current directory is not a Git repository."
IF branch empty OR branch IN {main, master}
  ERROR "A PR cannot be created from detached HEAD or main/master."
IF docs_audit given AND the file is missing: ERROR "The supplied docs audit does not exist."
IF verification given AND the file is missing or contains a line that is not a `- [x] ` item:
  ERROR "The supplied verification must contain only `- [x] ` lines."

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

========== REBASE (ONLY IF NEEDED) ==========

IF `git merge-base --is-ancestor origin/master HEAD` fails:
  Invoke `rebase-origin-main` with the Bus adapter base `origin/master`.
  It owns conflict triage, dependency/submodule resync when applicable, and force-with-lease push; this skill never runs rebase itself.
  If it reports a non-empty `dropped_shared_infra`, it must be written verbatim into the PR description's `## 其他说明`; never omit it.
Never describe or publish a pre-rebase tree.

baseline = `git rev-parse origin/master`; head = `git rev-parse HEAD`.
Assert the remote branch tip is exactly `head`.
copy_diff = `git diff <baseline>...<head>`; copy_commits = `git log <baseline>..<head> --oneline`.

IF verification given AND any line names a commit other than `head`:
  STOP and report the stale evidence; never publish verification that does not prove `head`.

========== LOCK PR INTENT ==========

IF plans nonempty:
  Run `python3 skills/pr/scripts/pr_goal_context.py --branch "<branch>" --output
  "<ignored-temp-goal-context>"`, appending `--plan "<path>"` once per plan in original order.
ELSE:
  Write a single goal from copy_diff / copy_commits; write only evidence-backed intentional exclusions as non-goals, otherwise write `无`.
  Write the two into ignored temp goal/non-goal files respectively, then Run:
    python3 skills/pr/scripts/pr_goal_context.py --branch "<branch>" \
      --output "<ignored-temp-goal-context>" --goal-file "<goal-file>" \
      --non-goal-file "<non-goal-file>"
IF command fails: STOP; never hand-parse plans or create the lock yourself.
Capture `GOAL_CONTEXT_FILE`, `LOCKED_GOAL_FILE`, and `LOCKED_NON_GOALS_FILE`; Read(GOAL_CONTEXT_FILE) completely.
The two sections of that context, `.locked-goal`, and `.locked-non-goals` are locked by the script and must not be rewritten afterwards.

========== WRITE BODY ==========

Read(references/pr-template.md)
Write the English title and narrative under one hard constraint: **the narrative may come only from the
immutable `copy_diff` / `copy_commits`; goal and non-goals come only from GOAL_CONTEXT_FILE.** Never re-derive
from the live worktree, live `HEAD`, or the mutable `origin/master`.

Fill every template section:
- Insert the full `## 目标` and `## 非目标` sections of GOAL_CONTEXT_FILE verbatim; never add source labels,
  summaries, translations, or separators. Same-named sections from multiple plans were already appended by the renderer in input order.
- `## 文档同步`: IF docs_audit given, run the following and insert stdout verbatim in the section's template
  position (do not hand-edit, translate, reorder, or infer):
    python3 skills/update-docs/scripts/docs_audit.py render-pr --audit "<docs_audit>"
  ELSE omit the whole section.
- `## 自测 / Agent 测`: IF verification given, insert its `- [x] ` lines verbatim and set phase = final.
  ELSE write one `- [ ] ` item stating that readiness verification was not provided to `pr`, and set phase = draft.
  Never claim gates, docs audits, E2E, review, or manual verification that the caller did not supply.
- When the diff touches delivery, callbacks or launch, note in `## 其他说明` that `just e2e` (live model usage)
  is pending unless the supplied verification already records it.

Write `body_file` under ignored `temp/`, then run:

  python3 skills/pr/scripts/pr_format_check.py --phase <phase> \
    --template skills/pr/references/pr-template.md --title "<title>" \
    --body-file "<body-file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF the check fails: fix and rerun; do not publish an unvalidated body.

========== PUBLISH ==========

existing = `gh pr list --head <branch> --state open --json number,title,url,isDraft`
STOP if it contains more than one PR. `created_as_draft = false`.
IF existing PR found (Draft or ready):
  `gh pr edit <number> --title "<title>" --body-file <body-file>`; never change its draft/ready status.
ELSE:
  `gh pr create --draft --head <branch> --base master --title "<title>" --body-file <body-file>`;
  `created_as_draft = true`.

Capture PR number/URL. A PR created here stays draft; never run `gh pr ready`.
The explicit PR request authorizes only this create/edit; never approve, merge, close, or delete
anything without separate authorization.

Then read back and verify the published state — `gh pr edit` can silently fail or truncate,
and this is the only check that proves what is actually on GitHub:
  published = `gh pr view <number> --json title,body,headRefOid,state,baseRefName,isDraft`
Bind returned fields as `published_title`, `published_body`, `published_head`, `published_state`,
`published_base`, and `published_is_draft`; write `published_body` byte-exact to ignored `temp/` as
`published_body_file`, and rerun `pr_format_check.py --phase <phase>` against it with `published_title`.

IF `published_title` or `published_body` differs from the intended title/body or fails the check
  Fix via `gh pr edit` and re-verify; do not report success with a non-conforming PR.
Require `published_head == head`, `published_state == OPEN`, `published_base == master`, and, for a
PR created this run, `published_is_draft == true`; otherwise STOP with the observed fields.
Assert the worktree has no change this skill made.

========== RETURN ==========

Report:
  - PR title and URL;
  - whether the PR was created as a Draft or an existing PR was refreshed (its draft/ready status unchanged);
  - head (the commit the body describes) and commits created or updated;
  - the phase used, and which optional inputs (`--verification`, `--docs-audit`) were supplied or absent;
  - `GOAL_CONTEXT_FILE`, plus `LOCKED_GOAL_FILE` and `LOCKED_NON_GOALS_FILE` for `review-pr` to consume;
  - pr_format_check result on the published PR;
  - verification still pending (gates, docs audit, E2E, manual) for the orchestrator to sequence;
  - unrelated dirty files explicitly excluded from the PR.
```
