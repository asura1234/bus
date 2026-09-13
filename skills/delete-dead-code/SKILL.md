---
name: delete-dead-code
description: Find and safely remove confirmed dead first-party Bus code, stale tests, or duplicate implementations within an explicit scope. Use when asked to remove dead code, zombie tests, unused code, or duplicate implementations.
---

# Delete dead code

Read [guide.md](guide.md) before deleting anything.

Input should name one or more directories. If it does not, derive candidate
directories from the owned `origin/main...HEAD` diff and show the resolved scope
before deletion. Never scan or edit `vendor/`, `.git/`, `target/`, `temp/`,
generated artifacts, or unrelated dirty files.

1. Resolve repository, branch, immutable base SHA, and scope. Refuse detached
   HEAD. Require the user's explicit authorization before changing a protected
   branch.
2. Record existing tracked and untracked changes. Do not require an otherwise
   dirty user worktree to be cleaned, but refuse any candidate file that
   overlaps unowned edits.
3. Inventory first-party definitions, re-exports, feature-gated/platform-gated
   code, tests, build scripts, integration assets, configuration, and external
   string/protocol references within the scope.
4. Treat compiler/linter warnings and text search as leads, not proof. Trace
   production ownership from entrypoints and manifests. Account for Rust macros,
   traits, `cfg` gates, `include_*`, serialization names, FFI, command strings,
   shell assets, and plugin/provider discovery.
5. Classify each candidate:
   - `CONFIRMED`: no supported production owner or intentional compatibility
     role remains; safe to delete or consolidate.
   - `KEEP`: reachable, public, dynamically referenced, platform-specific, or
     intentionally redundant.
   - `FLAG`: evidence is incomplete or ownership is disputed.
6. Write the findings ledger described in
   [references/dead-code-findings-format.md](references/dead-code-findings-format.md)
   under `temp/delete-dead-code/`.
7. Delete or consolidate only `CONFIRMED` candidates. Do not rewrite assertions
   to make stale tests pass. Rust tests include inline `#[cfg(test)]` modules and
   `assert!`, `assert_eq!`, `assert_ne!`, `matches!`, and panic assertions.
8. Validate the smallest affected test/build surface, then run `just lint`. Run
   `just ci` when preparing a PR or when deletion crosses integration boundaries.
9. Re-scan references and inspect the final diff for accidental scope expansion.
10. Commit only if the user asked; use `commit-and-push` for landing.

If deletion changes a public or persisted contract, stop: that is a migration or
product change, not dead-code cleanup.
