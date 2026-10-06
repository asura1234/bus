---
name: update-docs
description: Recursively audit and update the AGENTS.md documents affected by changed Git leaves (the skills/AGENTS.md workflow SOT and vendored subtree contracts, including CLAUDE.md symlinks); use when preparing a PR or when explicitly asked to sync documentation.
---

# Update Docs

```
-- Rules
- Read `guide.md` and `references/docs-audit-format.md` completely before starting.
- Mechanical discovery, status changes, validation, and PR section rendering only call
  `scripts/docs_audit.py`; never hand-rebuild its protocol.
- This skill does not commit or push; when called by `pr`, Git landing remains owned by `pr`.

========== PREPARE ==========

repo = `git rev-parse --show-toplevel`
base = caller-provided `--base`, otherwise `origin/master`

Run:

  python3 skills/update-docs/scripts/docs_audit.py prepare \
    --repo "<repo>" \
    --base "<base>"

Capture the single stdout path as `audit`. Read the generated `audit.json`,
`changed-files.txt`, and `targets.txt` completely. Every target is an existing `AGENTS.md`:
`skills/AGENTS.md` carries workflow architecture, and the nearest vendored `AGENTS.md` ancestors
keep their upstream subtree contracts. Public behavior and release documentation are inspected
semantically from the changed leaves even when they are not `AGENTS.md` targets.

-- Whole-repo scope (no Git diff): pass `--all-modules` to prepare instead of relying on the
-- diff walk. Targets become every existing AGENTS.md.

========== RECONCILE ==========

FOR each pending target in audit order
  Apply the semantic audit procedure from `guide.md` for the target's kind (first-party or vendored).

  IF a first-party target directory's `CLAUDE.md` is missing or is not a symlink
    Fix it mechanically: `ln -sf AGENTS.md <dir>/CLAUDE.md` (before deleting a non-link file,
    explicitly confirm it holds no hand-written content). Vendored `CLAUDE.md` files stay as upstream ships them.

  IF current as-built facts require content changes
    Edit the target and choose status `created` or `updated`.
  ELSE
    Do not touch the file and choose status `verified-current`.

  Run:

    python3 skills/update-docs/scripts/docs_audit.py record \
      --audit "<audit>" \
      --path "<target>" \
      --status "<status>" \
      --reason "<short factual reason>"

STOP if a target cannot be audited from the current tree or overlaps another active owner. Leave it
pending and report the exact target; never guess or mark it verified.

Independently inspect whether changed user-visible behavior requires README, docs/next, website,
changelog, or integration documentation. Edit only the documents genuinely affected by this change
and preserve translation parity where the repository requires it.

========== VERIFY ==========

Format only documents changed by this run.

Run the directly affected docs, maintenance, or integration-asset tests, then the global gates:

  just docs-contract-test

IF a global gate fails only on unrelated pre-existing/task-owned documents
  Preserve the exact failure as an exclusion; do not edit outside this run's targets.
ELSE IF the failure was created by this run's own changes (including mapper-rule changes)
  Fix it, rerun the affected gate, and update its audit reason if needed.

Build repeated `--verification` arguments from commands that actually passed and repeated
`--exclusion` arguments from preserved pre-existing failures. Then run:

  python3 skills/update-docs/scripts/docs_audit.py finalize \
    --audit "<audit>" \
    --verification "<command: result>" \
    [--exclusion "<pre-existing failure>"]...

The command must fail while any target is pending or the artifact is malformed.

========== RETURN ==========

Report the audit path, target counts by final status, verification results, exclusions, and explicit
confirmation that no commit or push was performed. Do not hand-summarize a malformed or unfinished
audit.
```
