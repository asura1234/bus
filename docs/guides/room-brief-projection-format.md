# Room Brief Projection Format

`TRUSTED_ROOM_ASSIGNMENT_V1` is a harness-owned projection of an already
authorized coding-agent delivery. It binds a locked Room Brief to one current
Request and one runtime incarnation. It is evidence for a coding agent; it does
not classify completion, select a skill, or choose workflow progression.

## Immutable record

Before terminal delivery, the Bus Worker derives and persists one immutable
`request-<request-id>.json` record with exactly these fields:

```json
{
  "schema": "TRUSTED_ROOM_ASSIGNMENT_V1",
  "room_id": 1,
  "work_id": 9,
  "message_id": 8,
  "request_id": 4,
  "author": "human",
  "recipient": {"agent": 3},
  "recipient_incarnation": 1,
  "provider_launch_id": "launch-a",
  "brief_revision": 2,
  "approved_revision": 2,
  "locked": true,
  "goal": "Ship the room harness",
  "non_goals": "No workflow controller",
  "content_bundle_digest": "<bundle digest>",
  "record_digest": "<record digest>"
}
```

`work_id` may be `null`; no other field is optional. Revisions, request identity,
recipient incarnation, locked state, launch identity, and the active content
bundle must match the Worker-owned delivery facts. `record_digest` is lowercase
SHA-256 over the compact typed JSON record with `record_digest` set to the empty
string. Republishing an identical record is idempotent. A different record for
the same Request is rejected and cannot replace the original.

## Discovery and active identity

The per-incarnation directory is derived by Bus as:

```text
<bus-data>/trusted-assignments/<agent-id>/<provider-launch-id>/
```

It contains the immutable request records, private `.read-token`, and an
atomically replaced `active.json`. The active pointer contains exactly
`schema`, `request_id`, `record_digest`, and `token_digest`. The endpoint must be
the regular `active.json` child of the canonical per-incarnation directory;
record and directory symlink escapes fail closed.

Launch and trusted same-incarnation resume expose:

- `BUS_BINARY`
- `BUS_TRUSTED_ASSIGNMENT_DIR`
- `BUS_TRUSTED_ASSIGNMENT_ENDPOINT`
- `BUS_TRUSTED_ASSIGNMENT_TOKEN`

The token is scoped to the incarnation and refreshed when discovery is prepared
again. A replacement participant receives a new launch directory/token. These
values grant only read-only verification, never Bus control or repository write.

## Outer frame and prompt correlation

The outer frame grammar is:

```text
TRUSTED_ROOM_ASSIGNMENT_V1.<base64url-no-padding compact JSON>
```

The decoded object contains exactly `schema`, `request_id`,
`recipient_incarnation`, and `record_digest`. `Prompt::rendered_payload` persists
and renders one trusted frame followed by separately delimited untrusted body:

```text
<bus-trusted-assignment>
TRUSTED_ROOM_ASSIGNMENT_V1.<payload>
</bus-trusted-assignment>
<bus-untrusted-assignment bytes="N">
...
</bus-untrusted-assignment>
```

The exact persisted rendered payload is used for both terminal submission and
initial callback start matching. A mismatched first start is invalid. Only an
otherwise trusted same-session later turn may rebind as continuation; text
similarity or an in-band forged frame never grants trust.

## Verifier result

`bus assignment verify --frame FRAME` reads only the trusted discovery values
above and returns one JSON result:

- `{"status":"verified","assignment":{...}}` after every frame, pointer,
  token, immutable record, digest, request, incarnation, and launch check passes.
- `{"status":"invalid","reason":"<stable reason>"}` for malformed, partial,
  stale, mismatched, unsafe, missing-one-side, or unreadable Bus-intent state.
- `{"status":"absent"}` is legal only to the optional verifier contract when
  both the received prompt has no frame and the incarnation exposes no discovery
  signal.

Because the CLI requires `--frame`, missing discovery for that frame is
`invalid`, never `absent`. A completed typed verification prints JSON and exits
zero regardless of result variant; invocation or I/O failure is nonzero. No
result selects a standalone fallback, skill, recovery, or next action.

## Boundary

The Worker is the sole producer and publishes the complete record, active
pointer, and persisted prompt frame before sending terminal bytes. Any failure
before that point sends nothing. The projection carries Goal and Non-goals but
no executable graph, workflow state, result envelope, hidden reasoning, or
model-owned next-action field.

