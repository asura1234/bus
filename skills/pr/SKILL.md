---
name: pr
description: Feature-branch PR workflow. Land uncommitted work, rebase onto origin/master, then create or refresh an English Draft PR and its body. It does not run gates or dead-code cleanup; the orchestrator sequences gate-and-fix and delete-dead-code itself and may hand their results to pr as optional --verification input. Use when the user asks to "open a PR", "create a PR", "submit a PR", "update a PR", or "generate a PR description".
---

# PR

Resolve `SKILL_DIR` to the directory containing this loaded SKILL.md.
All host operations use `python3 "$SKILL_DIR/../_bus/cli_extensions/pr_host.py"` (`pr_host`).
Run `pr_host host` first; stop on an unknown host. For Codeup, read `../_bus/docs/codeup-pr-workflow.md`.

```text
INPUT $ARGUMENTS = [--plan <plan-file>]... [--origin <file>] [--relock <file>] [--lock-only] [--verification <file>]
plans = every plan path the developer supplied this time, in order of appearance; never guess from diff, commits, or historical state.
origin = optional file holding the PR's originating request verbatim: the developer's own words or the issue text,
  usually passed by the orchestrator. It is what the goal is locked from when there is no plan.
relock = optional file holding, verbatim, the question that names a change to the locked goal (widening or narrowing) and
  the developer's explicit answer to it.
lock-only = lock the goal and return without publishing; the orchestrator's lock-before-fan-out entry, valid on a branch with no work yet.
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
base_branch = `git symbolic-ref --short refs/remotes/origin/HEAD` without `origin/`; if unset, resolve the
remote default with `git ls-remote --symref origin HEAD`, and stop if it is unavailable.

IF repo missing
  ERROR "The current directory is not a Git repository."
IF branch empty OR branch IN {main, master}
  ERROR "A PR cannot be created from detached HEAD or main/master."
IF verification given AND the file is missing or contains a line that is not a `- [x] ` item:
  ERROR "The supplied verification must contain only `- [x] ` lines."

Run:
  git status --short
  git fetch origin
Assert `origin/<base_branch>` resolves.

working_changed = tracked + untracked files in the current worktree
committed_changed = `git diff --name-only origin/<base_branch>...HEAD`
ahead = `git rev-list --count origin/<base_branch>..HEAD`

IF NOT lock-only AND working_changed empty AND committed_changed empty AND ahead == 0
  ERROR "The current branch has no work to commit or publish relative to origin/master."

Inspect the complete current diff, untracked files, and commits. Confirm they describe one coherent
PR purpose. Do not silently discard, stash, rewrite, or absorb unrelated developer changes.

========== LOCK PR INTENT ==========

IF `temp/review-pr/<branch_slug>/.locked-goal` and `.locked-non-goals` already exist (an earlier run or the orchestrator locked them):
  IF relock is given:
    Rewrite the goal and non-goals to exactly the change the question names and the answer approves, and write the
    origin as the recorded `.locked-origin` (if any), a blank line, then relock verbatim; pass all three. The script
    refuses a changed lock whose origin does not append to the recorded one.
  ELSE:
    Reuse them unchanged: pass them as `--goal-file` / `--non-goal-file` with no `--origin-file`; the script keeps the
    recorded origin, and accepts a lock from before origins existed only unchanged. Only relock re-locks.
ELSE IF plans nonempty:
  Run `python3 "$SKILL_DIR/../_bus/cli_extensions/pr_goal_context.py" --branch "<branch>" --output
  "<ignored-temp-goal-context>"`, appending `--plan "<path>"` once per plan in original order
  (and `--origin-file "<origin>"` when origin is given).
ELSE:
  IF origin is not given:
    ask the developer for the PR's originating request (their words or the issue), its one-sentence goal, and its
    non-goals (`无` when none). Write the goal and non-goal replies verbatim as the goal and non-goals, and the request
    followed by both replies verbatim as the origin. Never fill any of them from the diff or commits.
  ELSE:
    write the goal as the origin's ask in one sentence, keeping its areas and items, and the non-goals the origin
    excludes (otherwise `无`).
  Write them into ignored temp files, then Run:
    python3 "$SKILL_DIR/../_bus/cli_extensions/pr_goal_context.py" --branch "<branch>" \
      --output "<ignored-temp-goal-context>" --goal-file "<goal-file>" \
      --non-goal-file "<non-goal-file>" --origin-file "<origin>"
IF command fails: STOP; never hand-parse plans or create the lock yourself.
Capture `GOAL_CONTEXT_FILE`, `LOCKED_GOAL_FILE`, `LOCKED_NON_GOALS_FILE`, and `LOCKED_ORIGIN_FILE`; Read(GOAL_CONTEXT_FILE) completely.
The two sections of that context, `.locked-goal`, `.locked-non-goals`, and `.locked-origin` are locked by the script and must not be rewritten afterwards.
IF lock-only: go to RETURN.

========== SCOPE CHECK ==========

It runs before anything is committed or pushed, so out-of-goal work never reaches the PR.
mb = `git merge-base origin/<base_branch> HEAD`.
pr_files = committed_changed plus the uncommitted files that belong to this PR (exactly what LAND will commit);
unrelated dirty files LAND leaves untouched are listed as excluded and never sorted.
Sort every file in pr_files (excluding `plans/**`) into: serves the locked goal, is a direct consequence of it (its
tests, docs, a required refactor), or serves neither.
Measure `git diff --shortstat <mb> -- <pr_files>`.
IF any file serves neither:
  STOP and report each such file with the purpose it serves, the locked goal, and the size; the caller chooses
  split-pr, moving it to the deferred list, or rerunning with `--relock` holding the developer's ruling.

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

IF `git merge-base --is-ancestor origin/<base_branch> HEAD` fails:
  Invoke `rebase-origin-main --base origin/<base_branch>`.
  It owns conflict triage, dependency/submodule resync when applicable, and force-with-lease push; this skill never runs rebase itself.
  If it reports a non-empty `dropped_shared_infra`, it must be written verbatim into the PR description's `## 其他说明`; never omit it.
Never describe or publish a pre-rebase tree.

baseline = `git rev-parse origin/<base_branch>`; head = `git rev-parse HEAD`.
Assert the remote branch tip is exactly `head`.
copy_diff = `git diff <baseline>...<head>`; copy_commits = `git log <baseline>..<head> --oneline`.

IF verification given AND any line names a commit other than `head`:
  STOP and report the stale evidence; never publish verification that does not prove `head`.

========== WRITE BODY ==========

Read(`$SKILL_DIR/../_bus/docs/pr-template.md`) and Read(`$SKILL_DIR/../_bus/docs/pull-request-template.md`)
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

  python3 "$SKILL_DIR/../_bus/cli_extensions/pr_format_check.py" --phase <phase> \
    --template "$SKILL_DIR/../_bus/docs/pr-template.md" \
    --body-template "$SKILL_DIR/../_bus/docs/pull-request-template.md" --title "<title>" \
    --body-file "<body-file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF the check fails: fix and rerun; do not publish an unvalidated body.

========== PUBLISH ==========

existing = `pr_host find --branch <branch>`
STOP if it contains more than one PR. `created_as_draft = false`.
IF existing PR found (Draft or ready):
  `pr_host edit --pr <number> --title "<title>" --body-file <body-file>`; never change its draft/ready status.
ELSE:
  `pr_host create --branch <branch> --base <base_branch> --title "<title>" --body-file <body-file>`;
  `created_as_draft = true`.

Capture PR number/URL. A PR created here stays draft; never run `gh pr ready`.
The explicit PR request authorizes only this create/edit; never approve, merge, close, or delete
anything without separate authorization.

Then read back and verify the published state — an edit can silently fail or truncate,
and this is the only check that proves what is actually on GitHub:
  published = `pr_host view --pr <number>`
Bind returned fields as `published_title`, `published_body`, `published_head`, `published_state`,
`published_base`, and `published_is_draft`; write `published_body` byte-exact to ignored `temp/` as
`published_body_file`, and rerun `pr_format_check.py --phase <phase>` against it with `published_title`.

body_matches = (`published_body` == the intended body)
IF host == codeup:
  body_matches = (`published_body.rstrip("\n")` == the intended body `.rstrip("\n")`)
IF `published_title` differs from the intended title, OR `body_matches` is false, OR the body fails the check
  Fix via `pr_host edit` and re-verify; do not report success with a non-conforming PR.
Require `published_head == head`, `published_state == OPEN`, `published_base == base_branch`, and, for a
PR created this run, `published_is_draft == true`; otherwise STOP with the observed fields.
Assert the worktree has no change this skill made.

========== RETURN ==========

Report:
  - PR title and URL;
  - whether the PR was created as a Draft or an existing PR was refreshed (its draft/ready status unchanged);
  - head (the commit the body describes) and commits created or updated;
  - the phase used, and whether optional `--verification` was supplied or absent;
  - for lock-only, only the lock lines below;
  - `GOAL_CONTEXT_FILE`, plus `LOCKED_GOAL_FILE`, `LOCKED_NON_GOALS_FILE` and `LOCKED_ORIGIN_FILE` for `review-pr` to consume;
  - the SCOPE CHECK result and the `--shortstat` size;
  - pr_format_check result on the published PR;
  - verification still pending (gates, E2E, manual) for the orchestrator to sequence;
  - unrelated dirty files explicitly excluded from the PR.
```
