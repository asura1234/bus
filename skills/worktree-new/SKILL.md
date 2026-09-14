---
name: worktree-new
description: Create a safe isolated Git worktree for Bus from an explicit base and optionally provision its Rust dependencies. Use when asked to create a worktree, isolated checkout, or parallel development copy.
---

# Create a worktree

Input is a name and optional explicit base ref. A worktree is an isolated
checkout, not a promise to delete its branch later.

1. Resolve the repository's main worktree from `git worktree list` even when
   invoked inside another worktree.
2. Require a non-empty name. Convert `/` to `-` only for the directory name;
   preserve the requested branch name. Use the `codex/` prefix by default when
   creating a new branch unless the user supplied an exact different name.
3. Resolve the base explicitly. Default to `origin/master` only when it exists.
   Never substitute remote HEAD or `upstream/master`. Fetch the selected remote
   and record the immutable base SHA.
4. Choose a sibling path `<main-worktree>-<safe-name>`. Refuse an existing path,
   existing worktree registration, or branch already checked out elsewhere.
5. Create the worktree non-destructively:

   ```sh
   git worktree add -b <branch> <path> <base-sha>
   ```

   If the exact requested branch already exists and is not checked out, use
   `git worktree add <path> <branch>` instead; never reset it to the base.
6. Do not copy secrets, local configuration, ignored files, build products, or
   `target/` from another worktree. Do not create LibTV submodule, `build/deps`,
   or memory links.
7. Ensure the tracked repository-local skill symlinks resolve in the new
   checkout. Do not run a separate skill installer.
8. Provision only when requested or required for the immediate task. Prefer
   cache-safe `cargo fetch --locked`; run a focused build/test rather than a
   heavy release build.
9. Return the absolute path, branch, base SHA, and any provisioning result.

If creation partially fails, remove only the newly created worktree registration
or empty destination after verifying its exact path. Never recursively remove a
broad or pre-existing directory.
