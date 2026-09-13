---
name: worktree-close
description: Safely remove a Bus Git worktree after checking dirty state and unpublished commits, while retaining its branch unless branch deletion is explicitly requested. Use when asked to close, remove, or clean up a worktree.
---

# Close a worktree

Input is an exact worktree path, basename, or checked-out branch. If omitted,
use the worktree containing the current directory.

1. Run `git worktree list --porcelain` and resolve exactly one target. Refuse an
   ambiguous match and refuse the first/main worktree.
2. Record the target path, branch or detached SHA, and Git common directory.
3. Inspect tracked changes, staged changes, untracked files, ignored files that
   look user-authored, and in-progress merge/rebase/cherry-pick state. Do not
   remove a worktree containing work that has not been explicitly discarded or
   preserved by the user.
4. Fetch the branch's configured remote when available. Identify commits not
   reachable from that remote branch and from the explicitly selected
   integration base. Content equivalence after squash is evidence to present,
   not automatic permission to delete the branch.
5. Show a concise deletion summary: worktree path, dirty/untracked state,
   unpublished commits, active operation, and whether a branch remains.
6. Obtain explicit confirmation before removing the worktree. Prefer ordinary:

   ```sh
   git worktree remove <exact-path>
   ```

   Use `--force` only after the user explicitly authorized discarding the exact
   residual state and the target has been revalidated.
7. Run `git worktree prune` and verify the path/registration is gone.
8. Keep the local branch by default. Delete it only when the user explicitly
   asks, after rechecking unpublished content. Remote branch deletion is a
   separate explicit action and must use an exact remote/refspec.
9. Report what was removed, what branch was retained or deleted, and whether any
   recovery point remains.

Never delete project files outside the resolved worktree path, and never remove
a worktree merely because its PR appears merged.
