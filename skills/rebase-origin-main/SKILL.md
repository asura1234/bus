---
name: rebase-origin-main
description: Rebase the current feature branch onto origin's default branch or a caller-provided base, triage conflicts by severity, then force-with-lease push with an explicit refspec. Use when the user asks to "rebase", "sync the default branch", "update to the latest default branch", "rebase onto a base", or "resolve conflicts and push". Agent-agnostic (claude / cursor / codex); Git plus target-prescribed dependency resync.
---
Resolve `SKILL_DIR` to the directory containing this loaded SKILL.md; keep the working directory in the target repository.
# Rebase onto the integration base

Rebase the current branch onto the latest origin default branch (or the caller's `--base <ref-or-commit>`), resolve conflicts, then force push.
Pure git; runnable by any AI agent.

```
-- Guard: refuse detached HEAD, main/master, and the origin default branch.
-- Remote: always `origin` (the target repository). Never fetch from, rebase onto, or push to `upstream`.

-- Rules
- Auto-resolve minor conflicts (imports, formatting, non-overlapping regions, docs/comments)
- Ask developer for major conflicts (same function modified both sides, signature changes,
  logic conflicts, delete-vs-modify)
- Conflict policy comes from target repository instructions. Do not copy lockfile/gitlink names from another repo.
- Report every shared-infra change intentionally dropped as dropped_shared_infra.

========== VALIDATE ==========

INPUT = [--base <ref-or-commit>]
repo = `git rev-parse --show-toplevel`; run Git commands in repo.
Run `python3 "$SKILL_DIR/scripts/repository_context.py" --repo "<repo>" --base HEAD`.
Read context.policy; STOP on failure. bus_adapter = policy.gate_command == "just ci".
default_ref = `git symbolic-ref --quiet refs/remotes/origin/HEAD` if available.
Use only the full `refs/remotes/origin/` prefix to derive a protected branch; never --short.

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
Run `python3 "<SKILL_DIR>/scripts/rebase_base.py" --repo "<repo>" --base HEAD --check-branch "<branch>"`.
IF it fails: STOP with stderr; the same guard protects fresh and paused invocations.

IF rebase_root exists:
  base = contents of `<rebase_root>/onto`, verified as a commit.
  IF --base was supplied:
    Resolve it with the command below and STOP if it differs from onto; never change a paused rebase's base.
  Run `python3 "$SKILL_DIR/scripts/submodule_state.py" check --repo "<repo>" --before "<pre_rebase_head>"`.
  STOP on dirty, unpushed or unrecorded child work before continuing the paused operation.
  Continue at `RESOLVE CONFLICTS` using the existing paused index/worktree.

========== FETCH & REBASE ==========

Run `python3 "$SKILL_DIR/scripts/submodule_state.py" check --repo "<repo>"`.
IF it fails: STOP before rebasing; preserve dirty, unpublished or unrecorded initialized-submodule work.
The child-work check examines all local child branches and a detached HEAD, excluding commits
reachable from child remote-tracking refs or from actual child-origin ref tips. A local tag or
successfully fetching a SHA alone does not prove publication. Only the recorded pin and current
HEAD are checked against remote ref tips; other local branches stay conservative unless covered
by remote-tracking refs. The check fetches missing tips in one tree-filtered batch when needed,
uses a temporary non-shallow object store for shallow clones, and fails closed.
Do not use the parent branch's publication as evidence that a child commit was published.
`git fetch origin`
Run `python3 "<SKILL_DIR>/scripts/rebase_base.py" --repo "<repo>"`, adding
`--base "<caller base>"` only when supplied, and always add `--refresh-default --check-branch "<branch>"`.
This discovers origin's live default with `git ls-remote --symref origin HEAD` and rechecks protection.
It persists the full symbolic origin/HEAD name so subsequent local guards protect the same default.
An explicit base needs no local tracking commit for that default; only its name is used by the guard.
Capture its sole stdout line as base. Never refresh/fetch when resuming a paused rebase.
IF resolution fails: STOP with stderr. Never substitute master/main for a missing origin HEAD.
The resolver uses origin's symbolic HEAD by default and pins the chosen base to a commit.

IF `git rev-list --count <base>..HEAD` == 0
  Print "The current branch has no commits of its own relative to <base>."
  -- Still continue: the branch may just be behind the base, and the rebase is a fast-forward.

-- Must be recorded before the rebase; PUSH's divergence check uses it to exclude "my own commits before rewriting".
--   Do not switch PUSH to reading ORIG_HEAD: it is a global single slot, and any merge/reset/pull
--   in between overwrites it.
pre_rebase_head = `git rev-parse HEAD`

`git rebase <base>`

IF no conflicts → skip to SYNC INITIALIZED SUBMODULES

========== RESOLVE CONFLICTS ==========

dropped_shared_infra = []

FOR EACH conflicted file (`git diff --name-only --diff-filter=U`)
  Read the file to understand both sides

  IF bus_adapter AND file is `Cargo.lock`:
    -- Preserve Bus's existing lockfile rule only inside its profile-selected adapter.
    `git checkout --ours -- Cargo.lock`
    `cargo metadata --format-version 1 > /dev/null`
    -- Re-add only manifest-required entries; never cargo update or hand-merge the lockfile.
    `git add Cargo.lock`
    append Cargo.lock to dropped_shared_infra

  ELSE IF the target policy explicitly requires taking this conflict from the integration base:
    IF file is a submodule gitlink:
      Run `python3 "$SKILL_DIR/scripts/submodule_state.py" take-base-gitlink --repo "<repo>" --path "<file>"`.
      -- This records stage 2 directly; checkout-ours + add can incorrectly record the child's old HEAD.
    ELSE:
      `git checkout --ours -- <file>`; follow target-prescribed regeneration, if any, then `git add <file>`.
    Record the path and the dropped branch change in dropped_shared_infra.

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

-- After taking <base>'s shared infra, this branch's commit may become entirely empty. git drops it
--   by default (a commit that only bumped Cargo.lock, once taken as <base>, no longer appears in log
--   after the rebase completes).
--   If git instead stops and asks for confirmation, use `git rebase --skip`; do not create an empty commit.

========== SYNC INITIALIZED SUBMODULES ==========

Run `python3 "$SKILL_DIR/scripts/submodule_state.py" sync --repo "<repo>" --before "<pre_rebase_head>"`.
It consumes policy.submodule_update: recursive checks out recorded pins for initialized paths only;
none performs no update and fails if a path still differs. It never initializes paths, forces checkout,
chases a branch tip or moves a child off unpublished/unrecorded work. Only old/new recorded pins are admitted.
Its before and after checks use the same child-work rule as the pre-rebase check.
Report any error and STOP before push.

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
--   <base>, only the same-name feature branch is pushed.
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
  - Resolved base commit and caller base or origin default branch used
  - Rebase result: number of commits replayed, the new HEAD, whether any skip happened
  - Conflict handling: files resolved automatically, files asked about
  - dropped_shared_infra: list each shared infra file taken as the <base> version,
    and state explicitly that this branch's changes to it were dropped
  - Divergence check: the value of `foreign`, and the number of commits judged to be "my own pre-rewrite
    versions" and dropped normally (i.e. `git rev-list --count HEAD..<remote_tip>` minus `foreign`) --
    say what the force push erased, even if all of it was your own
  - Push result and the `<src> -> <dst>` arrow
```
