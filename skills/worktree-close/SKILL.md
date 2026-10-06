---
name: worktree-close
description: Remove a git worktree that is no longer needed. Before removal it explicitly checks the worktree for uncommitted changes and unpushed commits, and only uses --force after the user confirms discarding them. PRs to this repo may be squash-merged, so whether a branch has landed is decided by content rather than ancestry; a local branch judged landed is deleted with -D without asking, one that has not landed or cannot be judged is kept and handed back, and the remote branch is deleted only when the user explicitly asks. Use when the user asks to "delete a worktree", "clean up a worktree", "close a worktree", "wrap up a worktree", or "I don't need this isolated copy anymore". Works across agents; mainly git, plus basic shell tools such as sed/du/mkdir/cp, and reads and writes the Claude memory directory. Argument — worktree name or path (defaults to the current worktree).
---

Remove a git worktree you no longer need. Mostly git, plus basic shell tools (sed/du/mkdir/cp) and a
read-write touch of the Claude memory directory; runnable by any AI agent.

Removing a worktree is NOT the same as retiring a branch. A worktree is just an isolated checkout —
it may be long-lived, may have been used for many branches, and may be on a detached HEAD. So this
skill removes the worktree, and decides the branch separately: a branch whose content is already in
origin/master is swept up automatically, anything else is kept and handed back to the user.

Target: $ARGUMENTS (uses the worktree containing the current directory if not provided)

```
========== RESOLVE TARGET ==========

TRY: `git worktree list`
IF failed
  ERROR "The current directory is not inside a git repository; cannot clean up a worktree."

main_worktree = path of the FIRST entry of `git worktree list`
`git -C <main_worktree> worktree prune`

-- Identify by worktree, not by branch: a worktree may be detached or hold any branch.
IF $ARGUMENTS is not empty
  worktree_path = the `git worktree list` entry whose PATH equals $ARGUMENTS,
                  else whose basename equals $ARGUMENTS,
                  else whose checked-out branch equals $ARGUMENTS
  IF none matched
    ERROR "No worktree found with name/path/branch $ARGUMENTS. Run git worktree list first to confirm."
  IF more than one matched
    ERROR "$ARGUMENTS matches more than one worktree; use the full path instead."
ELSE
  worktree_path = the `git worktree list` entry containing `pwd`
  IF none
    ERROR "The current directory is not inside any worktree; provide a worktree name or path."

IF <worktree_path> is <main_worktree>
  ERROR "Cannot remove the main worktree."

branch = branch checked out in <worktree_path> (may be empty for a detached HEAD)

is_inside_target = (`pwd` is <worktree_path> or below it)
IF is_inside_target
  Print "⚠️ Running inside the target worktree; this directory will no longer exist after removal."

========== CHECK FOR UNSAVED WORK ==========

-- Bus has no submodules, so a plain `git worktree remove` refuses a worktree with modified or
--   untracked files. That refusal covers only the working tree, though: it says nothing about
--   COMMITS that exist only here, and an in-progress merge/rebase/cherry-pick can look clean.
--   These explicit checks are the guard against losing work. Never skip them and never jump
--   straight to --force.

dirty_super = `git -C <worktree_path> status --short`
in_progress = any of MERGE_HEAD / CHERRY_PICK_HEAD / REVERT_HEAD / rebase-merge / rebase-apply
              under `git -C <worktree_path> rev-parse --git-dir`

-- Commits that exist only here are lost too, since this checkout is about to disappear.
-- `--not --remotes` asks the exact question that matters: which commits are reachable from HEAD but
--   from NO remote-tracking ref. Do NOT use `@{upstream}..HEAD`: a worktree-new branch has no
--   upstream on purpose (--no-track), and that command then FAILS, which reads as "nothing
--   unpushed" and would green-light discarding every commit on a brand-new branch. Do NOT use
--   `origin/master..HEAD` either: it false-alarms on a detached worktree or a side branch whose
--   commits are pushed but simply not merged to master (verified — it reported 6 safe commits).
-- CAVEAT: after a PR is squash-merged the remote branch is usually deleted, and once any
--   `fetch --prune` has run there is no remote-tracking ref left holding those commits, so this
--   prints the branch's ENTIRE history as "unpushed" for work that is fully landed. Before reporting
--   it, run merged_by_content(<branch>) from the BRANCH section; when it says MERGED, say
--   "already squash-landed on origin/master; these commits will not be lost" instead of raising the alarm.
unpushed_super = `git -C <worktree_path> log --oneline HEAD --not --remotes`

needs_force = false
IF any of the three is non-empty
  Print "⚠️ <worktree_path> has work that would be discarded:"
  Print the non-empty results, labelled (uncommitted / operation in progress / unpushed)
  Print "Force removal permanently loses the above. Confirm to continue?"
  -- STOP and wait for explicit user confirmation. Do NOT proceed on your own judgement.
  needs_force = true after the user confirms

========== RESCUE CLAUDE MEMORY ==========

-- Claude Code keys auto-memory by ABSOLUTE project path (~/.claude/projects/<slug>/memory/, slug =
--   abs path with every "/" → "-"). worktree-new symlinks the worktree's memory dir to the main
--   repo's, so normally there is nothing to rescue. But a worktree created BEFORE that linking
--   existed holds a REAL memory dir that would be silently orphaned once this worktree is gone.
-- (Codex needs nothing: ~/.codex/memories is a single global store.)

main_slug = <main_worktree> abs path with every "/" replaced by "-"
wt_slug   = <worktree_path> abs path with every "/" replaced by "-"
main_mem  = ~/.claude/projects/<main_slug>/memory
wt_mem    = ~/.claude/projects/<wt_slug>/memory

IF <wt_mem> exists AND is a real directory (NOT a symlink) AND holds any *.md
  `mkdir -p "<main_mem>" && cp -n "<wt_mem>"/*.md "<main_mem>"/ 2>/dev/null`
  Print "Merged the worktree's Claude memory files into the main repo's shared store (MEMORY.md index conflicts need a manual check)."

========== REMOVE THE WORKTREE ==========

IMPORTANT: Do NOT `cd`. Use `git -C <main_worktree>` for every git command from here on.

IF needs_force
  `git -C <main_worktree> worktree remove --force <worktree_path>`
  -- One --force is enough; a second invocation errors with "is not a working tree".
ELSE
  `git -C <main_worktree> worktree remove <worktree_path>`
  -- If this still refuses, something changed since the check: go back to CHECK FOR UNSAVED WORK.
-- This also removes .git/worktrees/<name>/ and the ignored target/ inside the worktree.

`git -C <main_worktree> worktree prune`

========== BRANCH ==========

-- Deleting the worktree does NOT retire <branch>; it stays a normal branch in the main repo.
--   Split by whether its work already landed:
--     LANDED     → delete it here, no question asked. Its commits are all reproducible from
--                  origin/master, so there is nothing to lose, and asking about a branch that
--                  provably costs nothing to delete is noise the user has to answer every time.
--                  `git reflog` still holds the old tip for a while if anyone wants it back.
--     NOT LANDED → keep it and hand it back. This is the case the safety gate exists for.
--     UNKNOWN    → keep it and hand it back. Never let an inconclusive check delete anything.
--   The REMOTE branch is never in scope here regardless — see below.

IF <branch> is non-empty
  -- Do NOT use `git branch --merged origin/master`, and do NOT use plain `git branch -d`: both answer
  --   "is <branch> an ANCESTOR of origin/master", and a squash-merged PR discards ancestry. So a
  --   fully landed branch reports "not fully merged" and -d refuses. Following that signal produces
  --   exactly the wrong two messages: it scares the user off deleting dead branches, and when they
  --   insist it claims -D "loses its unique commits" about work that is already in master. Ask by
  --   CONTENT instead.
  merged = merged_by_content(<branch>)   -- see below; three tiers, first conclusive one wins.
                                         --   MERGED | NOT_MERGED | UNKNOWN

  IF merged is MERGED
    -- -D, not -d: -d asks the ancestry question, so under squash merge it refuses every landed
    --   branch. Do NOT report this as a forced/risky delete; nothing unique is being discarded.
    `git -C <main_worktree> branch -D <branch>`
    Print "Deleted local branch <branch> (content already landed on origin/master; evidence: T<n>)"

  ELSE
    -- Keep it. Do not delete, and do not ask again later in the run.
    IF merged is NOT_MERGED
      Print "Kept branch <branch>: the following commits are not found in origin/master and could only be recovered via reflog after deletion:"
      Print the commits that tier 2 marked `+`
    ELSE
      Print "Kept branch <branch>: cannot mechanically confirm whether it landed (T1/T2/T3 all inconclusive); your call."
    Print "Say the word if you want it deleted."
    -- Only on an explicit request: `git -C <main_worktree> branch -D <branch>`

  IF the user ALSO asks to delete the remote branch AND
     `git -C <main_worktree> ls-remote --heads origin <branch>` is not empty
    -- Never bundle this into the local delete, not even when merged is MERGED: the local branch is
    --   this machine's disposable copy, the remote one is everyone's. Explicit request only.
    -- Only ever `origin`; never touch the `upstream` remote.
    `git -C <main_worktree> push origin --delete <branch>`

-- merged_by_content(<branch>) → MERGED | NOT_MERGED | UNKNOWN
--   Run the tiers in order and stop at the first CONCLUSIVE answer:
--   T1  `git -C <main_worktree> diff --quiet origin/master <branch>`
--         exit 0 → MERGED. Trees are identical, so the branch adds nothing master lacks. Conclusive
--         on its own; a non-zero exit is NOT a "no" — it also fires when the branch is merely
--         BEHIND master (verified: a landed branch 15 PRs behind showed a 400-file diff that was
--         entirely master's later work). Never report that number as the branch's own changes.
--   T2  `git -C <main_worktree> cherry origin/master <branch>`
--         compares patch-ids, so it sees through squash for a branch whose landed changeset is one
--         commit. All lines `-` → MERGED. Any `+` → keep those lines, they are the candidate
--         unique commits, and continue to T3 (a multi-commit branch squashed into one shows `+`
--         for every commit even though it landed).
--   T3  MB = `git -C <main_worktree> merge-base origin/master <branch>`
--         Compare `git diff --stat <MB> <branch>` against the squash commit on origin/master that
--         claims this work (find it by PR subject). Identical file list and identical +/- totals →
--         MERGED. Then spot-check per file: `git diff --quiet <branch> origin/master -- <file>`;
--         files that differ are expected where later PRs touched them (CHANGELOG.md, docs/next/),
--         so a differing file is not by itself evidence the branch did not land.
--   No tier conclusive → UNKNOWN. Say so and keep the branch; never default to either answer,
--     and never let UNKNOWN take the auto-delete path.

`git -C <main_worktree> fetch --prune origin`

========== OUTPUT ==========

Print:
  "Removed worktree: <worktree_path>"
  IF <branch> non-empty AND not deleted:  "  Kept branch <branch> (<NOT_MERGED ? "has unlanded commits" : "landing status unconfirmed">)"
  IF local branch deleted:                "  Deleted local branch: <branch> (content landed on origin/master)"
  IF remote branch deleted:               "  Deleted remote branch: origin/<branch>"
  "  Disk reclaimed: <df difference before/after removal, if measured>"
  ""

`git -C <main_worktree> worktree list`

IF is_inside_target
  Print ""
  Print "Next steps:"
  Print "1. Exit the current AI session"
  Print "2. In a terminal, switch to the main worktree: cd <main_worktree>"
  Print ""
```
