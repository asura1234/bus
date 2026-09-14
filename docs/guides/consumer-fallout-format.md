# Consumer Fallout Artifact Format

This document is the format source of truth for the complete `consumer_fallout.py` audit artifact and its default high-confidence summary. The tool proves only parseable textual reference relationships. The plan reviewer still decides, from behavior and contracts, whether a file actually requires modification.

## Complete artifact

With `--output <path>`, the script atomically writes JSON:

```text
schema_version = 2
kind = plan-consumer-fallout-inventory
plan = repo-relative plan path
note = scope disclaimer
actionable_summary = bounded task summary
high_confidence_summary = complete high-confidence task buckets
entries = complete low-confidence inventory
```

`entries` retains every direct-reference and same-name-test candidate for each file produced by the plan. It is evidence for targeted inspection and must not be loaded wholesale into reviewer context.

## High-confidence relationships

`high_confidence_summary` contains only:

- `direct-reference-and-same-name-test`: one candidate both references a produced file directly and is its same-name test.
- `one-hop-consumer-test`: a production consumer references a produced file directly and that consumer has a same-name test.

Every item contains:

```text
source
candidate
relation
chain
evidence
source_task_ids
candidate_owner_task_ids
declared_in_file_contract
resolution
```

`resolution` is one of:

- `requires-gate-review-or-explicit-exclusion`
- `requires-file-contract-or-explicit-exclusion`
- `requires-owner-or-explicit-exclusion`

Ownership and file-contract inclusion show only that a path is assigned; they do not prove a task gate runs it. The script never marks a candidate resolved automatically, interprets a text relationship as a required modification, or invents an exclusion.

## Default stdout summary

With `--output`, stdout is bounded JSON with `kind=plan-consumer-fallout-summary`:

- global `high_confidence_count` and `unresolved_count`;
- `limit_per_task`;
- per-task `high_confidence_count`, `resolved_count`, `unresolved_count`, and `omitted_unresolved_count`;
- at most `limit_per_task` unresolved items per task, ordered with `direct-reference-and-same-name-test` first and then by a stable chain ordering;
- each item includes only `relation`, `chain`, and `resolution`; detailed evidence is read from the artifact only for the relevant task.

`omitted_unresolved_count > 0` is not automatically a finding. It authorizes only targeted reading of the relevant task bucket when the bounded summary shows a real fallout risk.

## Reviewer disposition

Every high-confidence unresolved candidate must be verified as exactly one of:

- the relevant path is in the file contract, covered by an appropriate stable owner, and exercised by the task gate;
- the path does not need modification and the plan contains an explicit, verifiable exclusion rationale.

Ownership is an exclusive concurrency boundary, not an exact affected-file allowlist. The tool must not narrow stable directory owners into enumerated files, but a broad owner cannot replace file-contract or gate closure. It also must not compute arbitrary-depth transitive dependency closure.
