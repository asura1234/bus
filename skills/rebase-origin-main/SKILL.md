---
name: rebase-origin-main
description: Rebase the current Bus feature branch onto the latest explicit origin/master, resolve safe conflicts, verify the result, and update its remote branch with force-with-lease. Use for rebase, sync master, or update from origin/master requests.
---

# Rebase onto origin/master

This workflow targets the Bus fork's `origin/master`. It never substitutes
`upstream/master` or a remote symbolic HEAD.

1. Resolve the repository and branch. Refuse detached HEAD, `main`, and
   `master`.
2. If a rebase is already active, inspect its original head and target. Continue
   that rebase only when it belongs to the current request; otherwise stop and
   report the paused state.
3. Inspect tracked and untracked changes. Do not stash, discard, or include
   unrelated work without explicit authorization. Require a clean worktree for
   a new rebase.
4. Fetch exactly `origin master` and verify `refs/remotes/origin/master` resolves.
5. Record the current branch tip and its remote tracking ref, if any.
6. Run `git rebase origin/master`.
7. Resolve only conflicts whose intent is unambiguous, such as imports,
   formatting, non-overlapping additions, or comments. Stop for same-function
   logic conflicts, API contract changes, delete/modify conflicts, or uncertain
   behavior. Do not automatically choose either side of `Cargo.lock`; resolve
   it consistently with `Cargo.toml` and regenerate only when needed.
8. After every resolution, stage explicit files and continue. Never skip a
   commit merely to make the rebase pass.
9. Verify no rebase state remains, the worktree is clean, and the branch is
   based on `origin/master`. Run focused checks for resolved files; use `just ci`
   when the conflict changed behavior or build inputs.
10. If the branch was previously published, update only the same remote branch:

    ```sh
    git push --force-with-lease=refs/heads/<branch>:<old-remote-sha> \
      origin <branch>:<branch>
    ```

Refuse the push if the remote branch moved unexpectedly. Never force-push
`main`, `master`, or any `upstream` ref.

Report old and new tips, conflicts resolved, checks run, and whether the remote
branch was updated.
