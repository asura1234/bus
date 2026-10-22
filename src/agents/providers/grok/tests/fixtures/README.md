# Lane B fixture provenance

- session-start.json and shutdown-stop.json are unchanged payloads from the offline Grok Build 1.0.5 capture in temp/grok-provider/offline-hook-payloads.jsonl (first session).
- updates.jsonl is the unchanged offline-updates.jsonl capture. It contains session/shutdown hook execution envelopes only, no model response or final boundary.
- Captured binary SHA-256: 3dfa7f04fbb5427a8fbead286591543aaecb478b3a0ab222c4329eca1a3b2f86.
- turn-events.jsonl and all test-generated turn records are SYNTHETIC examples of fields documented by the binary-matched 10-hooks.md. They do not prove authenticated completion, native wakes, or the model-turn transcript schema.
- No authenticated model turn was run. A committed turn-final transcript schema and session/turn ownership evidence remain required before native success settlement can be enabled.

## Internal completion contract

The integrator's lane addendum approves CompletionAttestation as a trusted
Bus-defined Rust value. It is not a native Grok record and has no Deserialize
implementation or JSON input path. Tests explicitly construct synthetic
attestations to exercise exact session/prompt matching, settled idle, lossless
long Unicode replies, explicit empty finals and reconciliation without Stop.

The native resolver supplies no attestation. Native Stop, lastAssistantMessage,
stopHookActive, ACP chunks, idle notifications, hook_execution success, invented
JSON fields and unknown turn markers cannot certify a final. Incomplete,
malformed, delayed, clipped or unowned transcript evidence remains pending or
fails validation. The bounded reader preserves complete Unicode content within
its two-MiB suffix; it cannot prove the ownership or final boundary of a chunk.

G still owns a verified native evidence producer and durable, bounded
session-level idle-prompt reconciliation. No real worker/orchestrator reply
capture, continuation-cap recovery or native background wake is accepted by
these synthetic tests.
