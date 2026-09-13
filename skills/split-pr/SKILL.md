---
name: split-pr
description: Split a multi-purpose Bus branch into independent single-purpose branches based on an explicit base, preserving commits and user changes safely. Use when asked to split a PR or divide a branch into multiple PRs.
---

# Split PR

1. Resolve the source branch and explicit base. Default to `origin/main` only
   when it exists; never infer the base from remote HEAD. Refuse detached HEAD
   or a source equal to the base branch.
2. Require all changes intended for splitting to be committed. Preserve
   unrelated untracked files and do not branch-switch over tracked dirty files.
3. Inspect `<base>...HEAD` by commit and file. Group by independently valuable
   purpose, not directory alone. Every group must include its own tests and must
   not rely on another proposed PR.
4. Present the groups, dependencies, shared-file conflicts, and proposed branch
   names. If genuinely independent grouping is impossible without redesign,
   stop and explain why.
5. Create each branch from the immutable base SHA in a separate worktree. Use
   `worktree-new` semantics and a `codex/` branch prefix unless the user gave
   exact names.
6. Reconstruct each group with cherry-pick or explicit path patches. Resolve
   shared-file edits by intent; never silently duplicate or drop a hunk.
7. Run focused tests for each branch and inspect that branch's diff against the
   base. Then invoke `pr` separately for each group when PR creation was asked.
8. Keep the source branch until all groups are verified and the user explicitly
   authorizes cleanup. Do not delete remote branches automatically.

Return a mapping from source commits/hunks to destination branches and note any
content intentionally left on the source branch.
