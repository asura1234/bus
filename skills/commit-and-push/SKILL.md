---
name: commit-and-push
description: Analyze the current git changes, split them into commits following "each commit does exactly one thing" with lowercase Conventional Commit subjects, and push with an explicit source:destination refspec. Works on a feature branch and pushes to the same-name branch on origin, never upstream; refuses to commit on master (local commits included) unless the developer explicitly asked to commit or push directly to master.
---

Analyze the current changes, split them into commits following "each commit does exactly one thing", then push to the remote branch. Pure git/gh; runnable by any AI agent (claude / cursor / codex).

```
-- Guard: refuse detached HEAD and the protected branch (master / main)
branch = `git branch --show-current`
IF branch empty
  Print "❌ commit and push is not allowed on a detached HEAD."
  STOP

IF branch IN {main, master} AND the developer did NOT explicitly ask to commit or push directly to master
  Print "❌ This repo forbids committing on master by default — **local commits are forbidden too**, not only pushes."
  Print "   Create a feature branch first and rerun: `git checkout --no-track -b feat/<desc> origin/master` (fix/ or chore/ likewise),"
  Print "   then, after landing, open a PR back to master with /pr."
  STOP
-- An explicit developer request to commit/push directly to master authorizes the normal commit and
--   `git push origin master:master`; every ownership and validation check below still runs. Never force-push master.
-- Only master/main is listed: it is the only protected branch in this repo. No positive
--   `feat|fix|chore/*` allowlist — the remote has normal working branches without those prefixes
--   (e.g. `codex/*`), and an allowlist would block them too. "Other public/shared branches" cannot be
--   judged mechanically; the developer gates them.

-- Rules
Commit subject is a lowercase Conventional Commit accepted by `python3 scripts/conventional_commits.py --message-file <message-file>`
  (types: the validator's ALLOWED_TYPES — feat, fix, perf, docs, ci, test, refactor, chore, release)
Never use `git add -A` or `git add .` — add files individually
Each commit should do exactly one thing:
  - one feature slice
  - one bug fix
  - one test-only change
  - one docs-only change
  - one tooling/config change
Do not mix unrelated docs, tooling, tests, generated files, and production behavior in the same commit unless they are required for the same logical change.
Do not commit pre-existing unrelated dirty worktree changes unless the user explicitly asked for them.

-- Gather info
status = `git status`                                      # never use -uall
short_status = `git status --short`
unstaged_diff = `git diff`
staged_diff   = `git diff --staged`
name_status   = union of:
  `git diff --name-status`
  `git diff --name-status --staged`
  `git ls-files --others --exclude-standard`

remote_contains_head = `git merge-base --is-ancestor HEAD refs/remotes/origin/<current-branch>`
  # an unborn HEAD or a missing remote ref both count as false

IF no changes AND remote_contains_head:
  Print "No changes to commit or push."
  STOP

IF no changes AND NOT remote_contains_head:
  Print "Working tree is clean, but the current HEAD is not yet contained in the same-name remote branch; skipping commit and continuing to the explicit push."
  Skip the commit loop and continue to Push below.

-- Analyze commit groups
Read the changed files and diffs.
Group changes by logical purpose, not by file extension alone.

Suggested grouping rules:
  - Production code and its directly required tests can be one commit when they prove the same behavior.
  - Pure test additions for existing behavior can be their own commit.
  - Docs/guide updates should usually be separate from code changes.
  - Lint/tooling/CI command changes should usually be separate from product code.
  - Mechanical renames/moves should be separate from behavioral edits when practical.
  - Generated files should be committed with the source change that generated them, if required.

Write an execution plan before staging:
  Commit 1:
    Purpose: <one thing>
    Files/hunks: <paths and, if needed, hunk descriptions>
    Message: <type>: <lowercase summary>
  Commit 2:
    ...

If one file contains unrelated hunks for different commits:
  Use `git add -p <file>` to stage only the intended hunk.
  If interactive staging is too risky, ask the user before proceeding.

-- Commit loop
FOR EACH planned commit group:
  Confirm working tree still contains the expected files.
  Stage only this commit group's files/hunks:
    `git add <file1> <file2> ...`             # specific files only
    or `git add -p <file>`                    # only when one file has mixed-purpose hunks

  Verify staged content:
    `git diff --staged --name-status`
    `git diff --staged`

  IF staged content includes unrelated changes:
    Unstage only the unrelated paths/hunks.
    Re-check staged diff.

  Compose commit message:
    title = "<type>: <lowercase summary>"   # optional (scope) and ! allowed per the validator
    body  = 2-3 sentence description if useful (reason, issue reference)
    Append the Co-Authored-By trailer per your agent's own convention (if it has one);
      Do NOT hardcode a model name.
  Validate: `python3 scripts/conventional_commits.py --message-file <message-file>`
    IF it rejects the subject: fix the subject and re-validate before committing.

  Run `git commit` with HEREDOC:
    <type>: <lowercase summary>

    Description (if needed)

    <Co-Authored-By trailer per agent convention, if applicable>

  Record commit hash and files included.

-- Push: ALWAYS origin + explicit refspec <branch>:<branch> (source and destination both pinned) + SSH/HTTPS fallback
-- Pinning the destination with an explicit refspec takes push.default out of the decision; even if the
--   branch's upstream was mistakenly set to origin/master, a bare `git push` cannot land the commits on master.
--   -u only sets the upstream to the same-name remote branch.
-- Never push to the `upstream` remote (the Herdr source) unless the user explicitly requests that exact remote.
push_cmd = `git push -u origin <current-branch>:<current-branch>`

TRY: run push_cmd
IF failed:
  current_url = `git remote get-url origin`
  IF current_url starts with "git@":
    alternate_url = HTTPS version  (git@github.com:org/repo.git → https://github.com/org/repo.git)
  ELSE:
    alternate_url = SSH version    (https://github.com/org/repo.git → git@github.com:org/repo.git)
  `git remote set-url origin <alternate_url>`
  TRY: run push_cmd
  IF failed:
    `git remote set-url origin <current_url>`   # restore original
    ERROR "Push failed via both SSH and HTTPS"

-- After pushing, check the `<src> -> <dst>` arrow in the output first: it must be `<branch> -> <branch>`.

-- Output
Print:
  - commit count
  - each commit title/hash/files
  - any remaining uncommitted files intentionally left out
  - push result
```
