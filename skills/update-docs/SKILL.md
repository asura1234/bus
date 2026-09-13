---
name: update-docs
description: Reconcile Bus documentation with an implemented change while preserving the repository's intentionally minimal fork notice. Use when asked to update docs or when preparing a PR whose user-facing behavior changed.
---

# Update docs

This skill audits documentation; it does not automatically create a broad docs
tree or overwrite the intentionally minimal root `README.md`.

1. Resolve an immutable base (default `origin/main` when it exists) and list the
   committed and owned uncommitted changes.
2. Classify user-visible effects: commands/flags, configuration, keyboard/UI
   behavior, agent/provider integration, data compatibility, installation,
   diagnostics, and release notes.
3. Locate existing authoritative documentation and help text with `rg`. In this
   fork, the root README remains only the Herdr-fork and inherited-license notice
   unless the user explicitly requests a larger README.
4. Prefer updating the closest existing source of truth: CLI `--help`, config
   comments/schema, integration manifests/assets, or an already tracked document.
   Do not recreate deleted `docs/`, root `AGENTS.md`, or root `CLAUDE.md` merely
   because LibTV's workflow had them.
5. Preserve the Apache-2.0 license and upstream attribution. Never claim a new
   license or remove notices without explicit legal direction.
6. Make the smallest accurate update. Describe current shipped behavior, not a
   plan or aspiration.
7. Validate examples and commands against the built CLI when practical. Run the
   narrow documentation or maintenance test that owns the changed artifact; use
   `just docs-contract-test` only when that tracked contract is in scope.
8. Report updated surfaces and any deliberate no-doc decision. Do not commit or
   push unless asked.
