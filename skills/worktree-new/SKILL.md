---
name: worktree-new
description: Create a new git worktree (an isolated working copy of the repo) and provision it so it can build, test and commit out of the box — create it from a freshly fetched origin/master without making origin/master its upstream, prefetch the locked Cargo dependencies (`cargo fetch --locked`, no compile). A worktree can be kept long-term as an isolated copy or used only for a while; it is not bound to any branch lifecycle. Use when the user asks to "create a worktree", "new worktree", "open an isolated copy", "worktree new", or "parallel development". Works across agents. Argument — name.
---

Create a new git worktree — an isolated working copy of this repo — and provision it so it can build,
test and commit out of the box.

A worktree is just another checkout. Keep it as long as it is useful: a permanent parallel lane, a
scratch copy for a risky experiment, or a short-lived one. You can switch branches inside it later
like in any checkout, so the name below only decides the directory and the initial checkout.

Rationale, step ordering and failure handling: [guide.md](./guide.md).

Name: $ARGUMENTS

```
========== VALIDATE ==========

IF $ARGUMENTS is empty
  ERROR "Provide a name. Usage: worktree-new <name>"

TRY: `git rev-parse --git-dir`
IF failed
  ERROR "The current directory is not inside a git repository."

-- Always resolve the MAIN worktree; this skill may be invoked from inside another worktree.
main_worktree = path of the FIRST entry of `git worktree list`
repo_parent   = dirname of <main_worktree>
repo_name     = basename of <main_worktree>
safe_name     = $ARGUMENTS with "/" replaced by "-"   # dir name only; a git ref keeps $ARGUMENTS
worktree_path = "<repo_parent>/<repo_name>-<safe_name>"

display_worktree_path = <worktree_path> with the user's home dir prefix replaced by "~"
Use <worktree_path> for all filesystem and git operations; use the display form only in output.

========== CREATE ==========

-- The integration base is origin/master. Never substitute a remote's symbolic HEAD or
--   upstream/master (`upstream` is the Herdr source, not this fork).
`git -C <main_worktree> fetch origin master`

-- Two worktrees cannot have the same branch checked out, so each one starts on its own ref.
-- Which ref is not important beyond this point: the user may switch branches inside it afterwards.

IF <worktree_path> directory exists
  IF `git -C <main_worktree> worktree list` contains <worktree_path>
    Print "Worktree already exists at <display_worktree_path>; skipping creation, reconciling provisioning."
  ELSE
    ERROR "Directory <display_worktree_path> exists but is not a valid worktree. Delete it manually or pick another name."

ELSE IF `git -C <main_worktree> branch --list $ARGUMENTS` is not empty
  IF that branch is already checked out in some worktree
    ERROR "Branch $ARGUMENTS is already checked out in <that worktree path>. Switch to that directory, or pick another name."
  -- Check out the existing branch as-is; never reset it to the base.
  `git -C <main_worktree> worktree add <worktree_path> $ARGUMENTS`

ELSE IF `git -C <main_worktree> ls-remote --heads origin $ARGUMENTS` is not empty
  -- Track the existing remote branch; do NOT fork a new same-named branch off origin/master.
  -- Tracking is correct here: upstream is the SAME-named remote branch, not master.
  `git -C <main_worktree> worktree add <worktree_path> -b $ARGUMENTS origin/$ARGUMENTS`

ELSE
  -- --no-track is REQUIRED. Without it git sets upstream to origin/master, and then a bare
  --   `git push` from this worktree targets master. Verified: without --no-track git prints
  --   "set up to track 'origin/master'".
  `git -C <main_worktree> worktree add --no-track -b $ARGUMENTS <worktree_path> origin/master`

========== PROVISION ==========

-- Run this on BOTH the freshly-created and the already-exists path; it is idempotent.
-- AI agents cannot persist a cwd. Use `git -C <path>` for git, and for cargo use a single
--   `cd <worktree_path> && cargo …` call. Never rely on a cwd surviving into a later call.

-- Never copy secrets, local configuration, ignored files, build products or `target/` from another
--   worktree. vendor/ is tracked, so the checkout already has it; there are no submodules to seed.

-- 1. BUILD PREP — downloads the locked Cargo dependencies without compiling.
--    Rationale: guide.md "Build-prep step in provisioning".

  `cd <worktree_path> && cargo fetch --locked`

  provision_ok = step 1 succeeded

  ON FAILURE: do NOT roll back — the worktree already exists. Report the failing step; never
    report success for a step that did not run. See guide.md "Failure handling".

========== LINK CLAUDE MEMORY ==========

-- Point the worktree's memory dir at the MAIN repo's, so one canonical store survives removal.
--   Why Claude needs this and Codex does not: guide.md "Why symlink the Claude memory directory".

main_slug = <main_worktree> abs path with every "/" replaced by "-"
wt_slug   = <worktree_path> abs path with every "/" replaced by "-"
main_mem  = ~/.claude/projects/<main_slug>/memory
wt_proj   = ~/.claude/projects/<wt_slug>
wt_mem    = <wt_proj>/memory

`mkdir -p "<main_mem>" "<wt_proj>"`
-- Fold in any real memories a pre-link session already wrote (no-clobber: canonical wins).
`if [ -d "<wt_mem>" ] && [ ! -L "<wt_mem>" ]; then cp -n "<wt_mem>"/*.md "<main_mem>"/ 2>/dev/null; rm -rf "<wt_mem>"; fi`
`ln -sfn "<main_mem>" "<wt_mem>"`

memory_linked = true on success

========== OUTPUT ==========

Print:
  "Worktree ready:"
  "  Path: <display_worktree_path>"
  "  Checked out: $ARGUMENTS (you can switch branches inside that directory at any time)"
  IF provision_ok:      "  Dependencies: locked Cargo dependencies fetched; ready for just test-one / just ci"
  IF NOT provision_ok:  "  Dependencies: ⚠️ provisioning failed (see above); enter the directory and finish it manually"
  IF memory_linked:     "  Memory: Claude memory symlinked to the main repo's shared store (survives worktree removal)"
  ""
  "Next steps:"
  "1. Exit the current AI session"
  "2. In a terminal, switch to the new worktree:"
  ""
  "   cd <display_worktree_path>"
  ""

`git -C <main_worktree> worktree list`
```
