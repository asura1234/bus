# Documentation Update Guide

## Goal

Keep `AGENTS.md` synchronized with the current implementation by walking from changed Git leaves to
every document that can describe them. There is exactly one documentation family, and in Bus it
comes in two kinds distinguished by where the file sits:

- **First-party `AGENTS.md`** (`skills/AGENTS.md`): the canonical skill root, shared guide and
  helper locations, repository adapters, validation commands, and registry-link contract. Every
  change under `skills/`, `docs/guides/`, `docs/templates/`, or `cli_extensions/` maps to it.
- **Vendored `AGENTS.md`** (the nearest existing ancestors under `vendor/`): upstream subtree
  contracts that keep their upstream scope and terminology.

Bus intentionally has no repository-root `AGENTS.md` or root `CLAUDE.md`; the walk never creates
one. Public behavior and release documentation are not `AGENTS.md` targets, but they are inspected
semantically from the changed leaves.

The walk is mechanical; deciding whether a document is still truthful is semantic.

## Target identity and the CLAUDE.md convention

Targets come from the Bus mapper in `scripts/docs_audit.py`: `skills/AGENTS.md` for the workflow
paths above, plus every existing `AGENTS.md` ancestor of a changed leaf. Do not copy Desktop-specific
`./run`, pnpm, Electron, submodule, or module-document rules into Bus.

Wherever a first-party `AGENTS.md` exists, `CLAUDE.md` must be a symlink to it
(`ln -s AGENTS.md CLAUDE.md`). This is a mechanical invariant; never copy content into a real
first-party `CLAUDE.md`. Vendored `CLAUDE.md` files are upstream content and stay as shipped.

## Sources of truth

Start from changed behavior, not from filenames alone. A documentation target is current only when
a reader can still derive the implemented contract, ownership boundary, supported command, and
failure behavior from it.

Use current source, tests, manifests (`Cargo.toml`, the `justfile`), skill entrypoints, helpers, and
existing documents. A plan describes intended work and may provide context, but it is not evidence
that the implementation exists.

For `skills/AGENTS.md`, inspect the whole current skill surface rather than the changed hunk alone:
skill directories and their entrypoints, shared guides, templates and `cli_extensions/` helpers,
repository adapters, validation commands, and the discovery-link registry.

For vendored `AGENTS.md`, preserve upstream scope and terminology. Do not rewrite vendored guidance
merely because first-party Bus code changed elsewhere.

For public and release documentation, inspect direct consequences of user-visible CLI,
configuration, socket, integration, install, and terminal behavior.

## Target rules

- An `AGENTS.md` target exists for `skills/AGENTS.md` whenever the workflow paths change, and for
  every ancestor directory of a changed leaf that already has an `AGENTS.md`.
- Deleted and renamed leaves remain audit inputs. A deleted subtree does not require recreating its
  documents, but any surviving document must remove or revise stale links and ownership claims.

## Editing standard

Documents describe the as-built tree, not aspirations. Preserve the repository's English/translation
parity rules.

`skills/AGENTS.md` reconciliation: keep it scoped to what agents working anywhere under `skills/`
must know — the canonical skill root, shared locations, repository adapters, validation commands,
and the registry-link contract. Do not restate individual skill internals that the skill's own
`SKILL.md` owns.

A source change that does not alter a documented contract should leave docs untouched and be
recorded as verified current. Use `updated` only when content changed, `created` only for a newly
created required document, and `verified-current` only after reading the full target surface and
confirming no edit is needed. Whitespace churn, synonym replacement, or reformatting solely to
create a diff is forbidden.

## Ownership and failures

Do not edit unrelated dirty work or a target currently owned by another active task. Stop with that
target pending so the caller can coordinate ownership. A pending target is preferable to a false
`verified-current` result.

The global gates may expose pre-existing failures outside the recursive target set. Record those
exact failures as exclusions and leave them untouched — unless this run's own changes created the
requirement (for example a mapper-rule change that makes a path newly require a document);
requirements caused by this run must be satisfied before finalization.

The mechanical gate proves paths, closed statuses, artifact completeness, and deterministic PR
rendering. It cannot prove the semantic audit was diligent; that remains the executing agent's
responsibility and is reviewable through each target reason and diff. If evidence is incomplete,
leave the audit pending rather than guessing.

## Git boundary

This workflow never commits or pushes. A standalone caller decides what to do with the resulting
worktree. The `pr` workflow consumes the finalized audit and owns commit, rebase, push, and PR
publication.
