---
name: rebase-origin-main
description: Rebase the current feature branch onto the latest origin/master, triage conflicts by severity, then force-with-lease push with an explicit refspec. Use when the user asks to "rebase", "sync master", "update to the latest master", "rebase onto origin/master", or "resolve conflicts and push". Agent-agnostic (claude / cursor / codex); pure git plus cargo for Cargo.lock.
---

# Rebase onto origin/master

Rebase the current branch onto the latest `origin/master`, resolve conflicts, then force push.
Pure git; runnable by any AI agent.

```
-- Guard: refuse detached HEAD and protected branches (main / master)
--   Same guard as `commit-and-push`: these are the protected branches in Bus.
-- Remote: always `origin` (the Bus fork). Never fetch from, rebase onto, or push to `upstream`.

-- Rules
- Auto-resolve minor conflicts (imports, formatting, non-overlapping regions, docs/comments)
- Ask developer for major conflicts (same function modified both sides, signature changes,
  logic conflicts, delete-vs-modify)
- Shared infra always takes the origin/master version on conflict; never merge it by hand:
    `Cargo.lock`
  The only regeneration allowed is cargo re-adding the entries this branch's own Cargo.toml
  still needs on top of origin/master's lockfile (see RESOLVE CONFLICTS); no `cargo update`.
  The cost is explicit: this drops this branch's own lockfile-only bumps. So every hit must be
  recorded in `dropped_shared_infra` and listed one by one in the final output, so the developer
  knows whether to redo it -- dropping is fine, dropping silently is not.

========== VALIDATE ==========

rebase_root = the single existing path from
  `git rev-parse --git-path rebase-merge` / `git rev-parse --git-path rebase-apply`

IF rebase_root exists:
  branch = strip `refs/heads/` from `<rebase_root>/head-name`
  IF branch empty
    ERROR "A rebase is in progress, but it has no named branch (rebase on a detached HEAD). Finish it manually."
  -- The resume path never gets a chance to record the old HEAD before the rebase, so read the
  --   orig-head the rebase itself stored. PUSH's divergence check needs it; without it there is no way
  --   to separate "my own commits before rewriting" from "commits someone else pushed".
  -- orig-head is a plain file holding one sha, not a ref: read its contents directly; `git rev-parse` cannot resolve it.
  pre_rebase_head = contents of `<rebase_root>/orig-head` (one 40-char sha)
  IF that file is missing or its contents are not a valid commit (verify with `git cat-file -e <sha>^{commit}`)
    ERROR "Cannot get the pre-rebase HEAD (<rebase_root>/orig-head missing or corrupt). Finish manually, then rerun."
  Do not fetch and do not start a second rebase.
ELSE:
  branch = `git branch --show-current`
  IF branch empty
    ERROR "Rebase is not allowed on a detached HEAD. Switch to a named feature branch first."

-- The protected-branch guard applies equally to both entry paths. The resume path also ends in the
--   final force push; checking only inside ELSE would let "a rebase paused on master" bypass the guard
--   and rewrite a protected branch directly.
IF branch IN {main, master}
  ERROR "Rebasing or rewriting history is not allowed on main/master."

IF rebase_root exists:
  Continue at `RESOLVE CONFLICTS` using the existing paused index/worktree.

========== FETCH & REBASE ==========

`git fetch origin`
Assert `origin/master` resolves.

IF `git rev-list --count origin/master..HEAD` == 0
  Print "The current branch has no commits of its own relative to origin/master."
  -- Still continue: the branch may just be behind master, and the rebase is a fast-forward.

-- Must be recorded before the rebase; PUSH's divergence check uses it to exclude "my own commits before rewriting".
--   Do not switch PUSH to reading ORIG_HEAD: it is a global single slot, and any merge/reset/pull
--   in between overwrites it.
pre_rebase_head = `git rev-parse HEAD`

`git rebase origin/master`

IF no conflicts → skip to PUSH

========== RESOLVE CONFLICTS ==========

dropped_shared_infra = []

FOR EACH conflicted file (`git diff --name-only --diff-filter=U`)
  Read the file to understand both sides

  IF file is `Cargo.lock`
    -- During a rebase ours/theirs are reversed: --ours = origin/master (the base being replayed onto),
    --   --theirs = your own commit being replayed. So "take the origin/master version" is --ours.
    --   Do not hand-edit it.
    `git checkout --ours -- Cargo.lock`
    -- If this branch's Cargo.toml adds or changes dependencies, origin/master's lockfile lacks them.
    --   `cargo metadata` resolves only what is missing and writes it back, without upgrading
    --   anything already locked. Never run `cargo update` here.
    `cargo metadata --format-version 1 > /dev/null`
    `git add Cargo.lock`
    append Cargo.lock to dropped_shared_infra

  ELSE IF minor conflict (import order, whitespace, formatting, non-overlapping edits, docs/comments)
    Resolve automatically
    `git add <file>`

  ELSE (same function/method modified by both sides, signature changes, logic conflicts,
        delete-vs-modify, uncertain)
    Show conflict content to developer
    AskUserQuestion — how to resolve?
    Apply developer's choice
    `git add <file>`

`git rebase --continue`
REPEAT until rebase completes

-- After taking origin/master's shared infra, this branch's commit may become entirely empty. git drops it
--   by default (a commit that only bumped Cargo.lock, once taken as origin/master, no longer appears in log
--   after the rebase completes).
--   If git instead stops and asks for confirmation, use `git rebase --skip`; do not create an empty commit.

========== PUSH ==========

IF `origin/<branch>` does not exist
  -- The branch was never pushed; there is nothing on the remote to erase, so no force and no divergence check.
  `git push -u origin <branch>:<branch>`
  Skip the rest of this section.

remote_tip = `git rev-parse origin/<branch>`

-- ⚠️ Do not use `git rev-list --count HEAD..origin/<branch>` to detect divergence. A rebase always rewrites
--   your own commits, so after **every** rebase of a pushed branch that count is > 0, and what it contains is
--   exactly your own pre-rewrite versions. Measured (scratch repo, two scenarios):
--     A remote has only your own rewritten commits → that count = 2 (pure false positive, fires on every rebase)
--     B remote really has one commit from someone else → that count = 3
--   Both are non-zero; it cannot tell the two apart, so "always reports" is equivalent to "blocks nothing".
--   The correct criterion is "on the remote, and neither on the new HEAD nor on the pre-rebase old HEAD":
--   under the same measurement A = 0, B = 1 (exactly the other person's commit).
foreign = `git rev-list --count <remote_tip> --not HEAD <pre_rebase_head>`
IF foreign > 0
  STOP, list `git log --oneline <remote_tip> --not HEAD <pre_rebase_head>` and report: the remote same-name
  branch has commits that were neither replayed in nor your own old versions, and a force push would erase
  them. Hand it to the developer (merge them in first and rerun, or explicitly authorize dropping them).

-- The explicit refspec pins both source and destination; even if the tracking branch is mistakenly set to
--   origin/master, nothing is pushed to master.
-- The lease carries the sha seen during the check above instead of letting it read the remote-tracking ref:
--   the `git fetch origin` at the start of this flow already moved the remote-tracking ref to the remote's
--   latest, so the default lease would use "what someone else pushed" as its expected value and never fail.
--   Measured: remote `[their commit, my commit]`; after a local fetch + rebase the push printed
--   `+ c4b8b5d...a43a102 feat/x -> feat/x (forced update)`, and their commit was gone. With <remote_tip>
--   pinned, anyone pushing between the check and the push gets the push rejected.
`git push --force-with-lease=<branch>:<remote_tip> origin <branch>:<branch>`

-- After the push, first check the `<src> -> <dst>` arrow in the output: it must be `<branch> -> <branch>`.
-- Lease rejected = the remote moved again after this check. Do not switch to --force; fetch again and rerun
--   from FETCH & REBASE, or report and let the developer confirm who owns those remote commits.

========== OUTPUT ==========

Print:
  - Rebase result: number of commits replayed, the new HEAD, whether any skip happened
  - Conflict handling: files resolved automatically, files asked about
  - dropped_shared_infra: list each shared infra file taken as the origin/master version,
    and state explicitly that this branch's changes to it were dropped
  - Divergence check: the value of `foreign`, and the number of commits judged to be "my own pre-rewrite
    versions" and dropped normally (i.e. `git rev-list --count HEAD..<remote_tip>` minus `foreign`) --
    say what the force push erased, even if all of it was your own
  - Push result and the `<src> -> <dst>` arrow
```
