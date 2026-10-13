---
name: pr
description: Feature-branch PR workflow. Land uncommitted work, rebase onto origin/master, then create or refresh an English Draft PR and its body. It does not run gates or dead-code cleanup; the orchestrator sequences gate-and-fix and delete-dead-code itself and may hand their results to pr as optional --verification input. Use when the user asks to "open a PR", "create a PR", "submit a PR", "update a PR", or "generate a PR description".
---

# PR

```text
INPUT $ARGUMENTS = [--plan <plan-file>]... [--origin <file>] [--verification <file>]
plans = every plan path the developer supplied this time, in order of appearance; never guess from diff, commits, or historical state.
origin = optional file holding the PR's originating request verbatim: the developer's own words or the issue text,
  usually passed by the orchestrator. It is what the goal is locked from when there is no plan.
verification = optional Markdown file of `- [x] ` lines, each naming verification actually executed and the commit it proves
  (for example a gate-and-fix PASS artifact with its Head); supplied by the caller, never produced here.
  A line that starts with a self-test box name of the GitHub template (`- [x] gate-and-fix — ...`,
  `- [x] e2e-test — ...`) is that box's evidence and must cite head `<sha>` and report `<path>`.

-- Rules
- Code narrative takes only the current repository, current feature branch, and actual changes as its source of truth; plans lock only goal and non-goals.
- Goal and non-goals come from the originating request (plan, origin, or the developer's answer), never from the diff or commits:
  a goal written from what was built always matches its growth, and then review-pr cannot catch the growth.
- Prior review-plan or review-pr invocation is not required.
- This skill never runs or requires `gate-and-fix` or `delete-dead-code`, or their artifacts. The orchestrator
  sequences them; their results reach the body only through `--verification`.
- Do not read or guess another skill's private control state, callbacks, or temporary artifacts; the only cross-skill
  handoff is `temp/review-pr/<branch>/.locked-goal`, `.locked-non-goals` and `.locked-origin`, written and returned by `pr_goal_context.py`.
- PR title and narrative are English; fixed headings and machine tokens stay byte-compatible with the format SOT:
  body sections, placeholders and self-test boxes live only in `.github/pull_request_template.md` (the body GitHub
  offers a person opening a PR); title rules and per-section writing guidance live in `references/pr-template.md`.
  Every published body must pass `pr_format_check.py`; never skip it.
- Every commit and push must be delegated to `commit-and-push`; every rebase must be delegated to `rebase-origin-main`; never duplicate their Git protocols.

========== PREFLIGHT ==========

repo = `git rev-parse --show-toplevel`
branch = `git branch --show-current`

IF repo missing
  ERROR "The current directory is not a Git repository."
IF branch empty OR branch IN {main, master}
  ERROR "A PR cannot be created from detached HEAD or main/master."
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

IF `temp/review-pr/<branch_slug>/.locked-goal` and `.locked-non-goals` already exist (an earlier run or the orchestrator locked them):
  Reuse them unchanged: pass them as `--goal-file` / `--non-goal-file`, plus `.locked-origin` as `--origin-file` when it exists.
  A refresh never re-locks; only the developer widens a goal.
ELSE IF plans nonempty:
  Run `python3 skills/pr/scripts/pr_goal_context.py --branch "<branch>" --output
  "<ignored-temp-goal-context>"`, appending `--plan "<path>"` once per plan in original order
  (and `--origin-file "<origin>"` when origin is given).
ELSE:
  IF origin is not given:
    ask the developer for the PR's originating request (their words or the issue), then its one-sentence goal and its
    non-goals (`无` when none); write the request verbatim as origin. Never fill any of them from copy_diff / copy_commits.
  Write the goal as the origin's ask in one sentence, keeping its areas and items, and the non-goals the origin excludes
  (otherwise `无`); write both into ignored temp files, then Run:
    python3 skills/pr/scripts/pr_goal_context.py --branch "<branch>" \
      --output "<ignored-temp-goal-context>" --goal-file "<goal-file>" \
      --non-goal-file "<non-goal-file>" --origin-file "<origin>"
IF command fails: STOP; never hand-parse plans or create the lock yourself.
Capture `GOAL_CONTEXT_FILE`, `LOCKED_GOAL_FILE`, `LOCKED_NON_GOALS_FILE`, and `LOCKED_ORIGIN_FILE`; Read(GOAL_CONTEXT_FILE) completely.
The two sections of that context, `.locked-goal`, `.locked-non-goals`, and `.locked-origin` are locked by the script and must not be rewritten afterwards.

========== SCOPE CHECK ==========

Sort every file in `git diff --name-only <baseline>...<head>` (excluding `plans/**`) into: serves the locked goal, is a
direct consequence of it (its tests, docs, a required refactor), or serves neither.
Measure `git diff --shortstat <baseline>...<head>`.
IF any file serves neither:
  STOP before WRITE BODY and report each such file with the purpose it serves, the locked goal, and the size; the
  caller chooses split-pr, moving it to the deferred list, or the developer's explicit yes to widen (which re-locks
  the goal with that yes added to the origin). Never publish over it.

========== WRITE BODY ==========

Read(references/pr-template.md) and Read(`.github/pull_request_template.md`)
Write the English title and narrative under one hard constraint: **the narrative may come only from the
immutable `copy_diff` / `copy_commits`; goal and non-goals come only from GOAL_CONTEXT_FILE.** Never re-derive
from the live worktree, live `HEAD`, or the mutable `origin/master`.

Fill every section of the GitHub template, in its order:
- Insert the full `## 目标` and `## 非目标` sections of GOAL_CONTEXT_FILE verbatim; never add source labels,
  summaries, translations, or separators. Same-named sections from multiple plans were already appended by the renderer in input order.
- `## 自测 / Agent 测`: keep every checkbox of the GitHub template (`e2e-test`, `gate-and-fix`), each once.
  A box whose evidence line verification supplies becomes that line verbatim; every other box stays
  `- [ ] <name> — pending`. Append the remaining verification lines verbatim after the boxes.
  phase = final when no box is pending, else draft.
  Never claim gates, E2E, review, or manual verification that the caller did not supply.
- When the diff touches delivery, callbacks or launch, note in `## 其他说明` that `just e2e` (live model usage)
  is pending unless the supplied verification already records it.

Write `body_file` under ignored `temp/`, then run:

  python3 skills/pr/scripts/pr_format_check.py --phase <phase> \
    --template skills/pr/references/pr-template.md \
    --body-template .github/pull_request_template.md --title "<title>" \
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
  - the phase used, and whether optional `--verification` was supplied or absent;
  - `GOAL_CONTEXT_FILE`, plus `LOCKED_GOAL_FILE`, `LOCKED_NON_GOALS_FILE` and `LOCKED_ORIGIN_FILE` for `review-pr` to consume;
  - the SCOPE CHECK result and the `--shortstat` size;
  - pr_format_check result on the published PR;
  - verification still pending (gates, E2E, manual) for the orchestrator to sequence;
  - unrelated dirty files explicitly excluded from the PR.
```
