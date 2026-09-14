# Docs Audit Format

`audit.json` is the strict, machine-validated artifact shared by `update-docs` and `pr`. It is
UTF-8 JSON with one trailing newline. Object keys are written in the order below. Every target is
an existing `AGENTS.md`: `skills/AGENTS.md` owns workflow architecture and vendored `AGENTS.md`
files retain their upstream subtree contracts. Bus intentionally has no repository-root AGENTS.md.

## Schema

```json
{
  "schemaVersion": 1,
  "complete": false,
  "base": "origin/master",
  "mergeBase": "0123456789abcdef",
  "changedLeaves": ["skills/example/SKILL.md"],
  "targets": [
    {
      "path": "skills/AGENTS.md",
      "status": "pending",
      "reason": ""
    }
  ],
  "verification": [],
  "exclusions": []
}
```

## Fields

- `schemaVersion` is exactly integer `1`.
- `complete` is boolean. It starts `false` and becomes `true` only through the finalizer.
- `base` and `mergeBase` are non-empty strings recording discovery identity.
- `changedLeaves` is the sorted, de-duplicated set of repository-relative Git paths. Absolute paths,
  `..`, empty paths, and duplicates are invalid.
- `targets` is the de-duplicated Bus mapper output, ordered by owning-directory depth from deepest
  to shallowest (an `AGENTS.md` belongs to the directory it sits in). Each `path` is unique and
  repository-relative.
- `status` is exactly one of `pending`, `created`, `updated`, or `verified-current`.
- `reason` is empty only while status is `pending`; every resolved target requires a concise factual
  reason.
- `verification` is an ordered array of non-empty `command: result` strings actually observed.
- `exclusions` is an ordered array of non-empty pre-existing/task-owned failures deliberately left
  untouched. No exclusions are represented by `[]`.

## State rules

- A prepared artifact has `complete: false`; all targets are `pending`; verification and exclusions
  are empty.
- Recording a target replaces its pending status and reason. Unknown paths, unknown statuses, empty
  reasons, and mutation after finalization are invalid.
- Finalization is invalid while any target is pending.
- A non-empty target set requires at least one verification entry.
- A finalized artifact has `complete: true` and contains no pending target.
- An empty target set is represented by `targets: []` and may finalize with `verification: []`.

## Complete example

```json
{
  "schemaVersion": 1,
  "complete": true,
  "base": "origin/master",
  "mergeBase": "0123456789abcdef",
  "changedLeaves": ["skills/example/SKILL.md"],
  "targets": [
    {
      "path": "skills/AGENTS.md",
      "status": "verified-current",
      "reason": "Responsibilities and boundaries remain accurate."
    },
    {
      "path": "vendor/libghostty-vt/AGENTS.md",
      "status": "updated",
      "reason": "Public exports and dependency direction changed."
    },
    {
      "path": "vendor/libghostty-vt/src/terminal/c/AGENTS.md",
      "status": "verified-current",
      "reason": "Repository stage summary remains accurate."
    }
  ],
  "verification": [
    "python3 -m pytest -q skills/update-docs/scripts/test_docs_audit.py: passed"
  ],
  "exclusions": []
}
```

## Deterministic PR rendering

`docs_audit.py render-pr` accepts only a complete valid artifact and emits:

```markdown
## 文档同步

- [x] `skills/AGENTS.md` — verified current
- [x] `vendor/libghostty-vt/AGENTS.md` — updated
- [x] `vendor/libghostty-vt/src/terminal/c/AGENTS.md` — verified current
```

For an empty target set it emits exactly one checked line: `No affected documentation targets`.
The consumer inserts renderer output verbatim and does not translate or reorder it.
