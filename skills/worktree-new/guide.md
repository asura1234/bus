# Worktree-New Guide

`SKILL.md` keeps only the executable steps; this file explains why those steps have their current
shape, and which parts must not be taken for granted.

## Build-prep step in provisioning

Step 1 only runs the build-prep step (`cargo fetch --locked`: download the dependencies pinned in
`Cargo.lock`) and does not compile. What a new worktree needs is "can build / test / commit", and
all three depend only on the dependencies being in place; a full compile is left to the developer
to run on demand (`just test-one <filter>` for focused iteration, `just ci` for the full gate).

`--locked` makes the fetch fail instead of silently rewriting `Cargo.lock`, so provisioning never
leaves an unrelated lockfile change in the new worktree. Do not copy `target/` from another
worktree: build products are per-checkout and a copied `target/` carries stale artifacts that point
at the other checkout's paths. `vendor/` is tracked and arrives with the checkout itself.

The fetch can be slow the first time (it fills the shared Cargo registry cache); that is normal.
Later worktrees reuse the same cache and finish quickly.

## Why symlink the Claude memory directory

Claude Code buckets auto-memory by **absolute path**: `~/.claude/projects/<slug>/memory/`, where
`<slug>` is the absolute path with every `/` replaced by `-`. So every worktree gets **its own**
memory directory, and whatever is written there becomes an orphan the moment the worktree is
removed.

So point the worktree's memory directory at the main repo's, letting the single canonical store
survive worktree removal. Fold in existing content with no-clobber (`cp -n`): for same-named files
the main repo's copy wins, so a worktree's temporary memory never overwrites the canonical version.

Codex does not need this step: `~/.codex/memories` is a single global store, not bucketed by path.

## Failure handling

A provisioning failure is **not rolled back** — the worktree has already been created by then.
Report which step failed and let the developer enter the directory and retry; never report success
for a step that did not actually run.
