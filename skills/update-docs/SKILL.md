---
name: update-docs
description: Audit a Bus diff from changed leaves to affected durable documentation, reconcile each target, and emit the canonical PR documentation section without committing or pushing.
---

# Update Docs

Read [guide.md](guide.md) and [docs-audit-format.md](references/docs-audit-format.md) completely. Use `scripts/docs_audit.py` for discovery, state changes, validation, and PR rendering; never hand-recreate its protocol.

```text
========== PREPARE ==========

repo = git rev-parse --show-toplevel
base = caller-provided --base, otherwise origin/master

Run:
  python3 skills/update-docs/scripts/docs_audit.py prepare \
    --repo "<repo>" --base "<base>"

Capture the sole stdout path as audit. Read audit.json, changed-files.txt, and
targets.txt completely. The Bus mapper follows only documentation SOTs that
actually exist: skills/AGENTS.md for workflow architecture and the nearest
vendored AGENTS.md ancestors for vendored code. Public behavior and release
documentation are inspected semantically from the changed leaves even when
they are not AGENTS targets.

========== RECONCILE ==========

FOR each pending target in audit order
  Apply the semantic procedure in guide.md.

  IF current as-built facts require a change
    Edit the target and choose created or updated.
  ELSE
    Leave the file byte-identical and choose verified-current.

  Run:
    python3 skills/update-docs/scripts/docs_audit.py record \
      --audit "<audit>" --path "<target>" --status "<status>" \
      --reason "<short factual reason>"

STOP when a target cannot be verified from the current tree or overlaps
another active owner. Leave it pending; never guess.

Independently inspect whether changed user-visible behavior requires README,
docs/next, website, changelog, or integration documentation. Edit only the
documents genuinely affected by this change and preserve translation parity
where the repository requires it.

========== VERIFY ==========

Format only documents changed by this run. Run the directly affected docs,
maintenance, or integration-asset tests, then the applicable repository docs
contract (`just docs-contract-test` or another existing exact recipe).

If a full gate fails only on unrelated pre-existing/task-owned files, preserve
the exact failure as an exclusion. Fix failures created by this run and rerun
the affected gate.

Build repeated --verification arguments only from commands that passed and
--exclusion arguments only from proven pre-existing failures. Then run:
  python3 skills/update-docs/scripts/docs_audit.py finalize \
    --audit "<audit>" --verification "<command: result>" \
    [--exclusion "<pre-existing failure>"]...

The command must fail while any target is pending or the artifact is malformed.

========== RETURN ==========

Report the audit path, target counts by status, verification, exclusions, and
explicitly confirm that this skill did not commit or push. Do not summarize an
unfinished or malformed audit.
```
