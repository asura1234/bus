# Statistics artifact contract

`stats.schema.json` is the field/type/enum source of truth. Python uses that schema
for every input and output record. Unknown keys, duplicate JSON keys, invalid
identities/timestamps, and unknown enum values fail closed. No ratings exist.

The store contains append-only `events.jsonl`, one UTF-8 record per line. An event
ID is SHA-256 of canonical JSON excluding `id` (sorted keys, compact separators,
Unicode preserved). Identical re-imports are idempotent; conflicting facts at the
same semantic identity stop collection. Atomic append holds a local file lock.

Every record carries agent and orchestrator identities. `unknown` is the sole
unavailable identity/model expression. Historical lane names remain names; an
unavailable Bus ID stays `unknown`. No current identity is retroactively applied.
Every source is a saved file with SHA-256 and a locator: a line range, JSON
pointer, named artifact section, or the recorded computation over referenced Git
objects. The hash binds the complete original source bytes.
Timestamps are UTC RFC3339 with zero or 3–6 fractional digits and `Z`; unavailable timestamps are null. A review
with only a calendar date keeps that date separately. Empty sets use `[]`.

Assignments record observed identity at append time. A handover names the old
agent and the orchestrator's reason. A message ID is optional; it links delivery
observations to the assignment. Retrying the same brief/task/role/agent keeps
the first assignment and returns its original ID, even across clock changes or
concurrent calls. A new nudge message remains a new assignment. Machine snapshots
keep their original content.

Canonical detailed review artifacts supply scope hashes. Review identity is
task/lane/scope/round/head; branch reviews have null scope. Older records recover
an explicit scope header from their checksum-verified saved source. Ambiguous
scoped finding references omit exact review attribution with a coverage reason.
Rendered chat summaries do not substitute for canonical detailed reviews.

Root causes come only from address-review-comments' canonical items. That skill
owns the [root provenance format](../../address-review-comments/references/root-cause-provenance-format.md)
and its public `parse_provenance` reader. This add-on only consumes that reader;
it neither defines the table contract nor decides which findings share a root.
Explicit historical prose source lists have a separate bounded legacy adapter.
Missing or ambiguous source mappings are coverage omissions.

Task phase events use automatically sampled UTC times (seconds or milliseconds)
and a monotonic clock. An end references its start ID and contains the computed
elapsed milliseconds. `task-end --artifact PATH` binds that exact artifact hash
to the assignment identity and completion time. `task-end` also returns an
immutable event receipt, usable by the runner for measured slot waiting.

For an applied root detected by a gate/live runner, the owning provenance format links its exact failure ID. The referenced report must
record that failure ID; the canonical claim must cite repository `path:line`.
These are links to existing triage evidence, not new adjudications. Escapes need
an observed Ready completion earlier than the linked detection, ancestry, and
identical cited Git blobs. Unknown ordering or changed blobs emits no escape.
Root credit is still per canonical claim and reporting reviewer, not per test.
