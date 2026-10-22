# Grok provider acceptance: synthetic fixtures and pending native evidence

All fake turns, final-boundary records, permission/question cards and wakes
below are **SYNTHETIC**. The observed 1.0.5 evidence establishes camelCase
session/shutdown hooks and the ACP update envelope only. It does not establish
native final markers, authenticated cards, or automatic background reentry.
Authenticated native acceptance remains pending.

## Local verification

Scrub inherited BUS_* / HERDR_* before builds and harnesses. Use an isolated
CARGO_TARGET_DIR, CARGO_INCREMENTAL=0 and nice -n 19; limit build and test
parallelism to fewer than half the machine's cores.

    nice -n 19 cargo nextest run --locked --bin bus --features dev-tools -E 'test(grok)'
    nice -n 19 python3 -m unittest tools.tests.grok_e2e_test tools.tests.grok_probe_test tools.tests.grok_fake_test

Run synthetic integration with an explicitly isolated dev-tools binary:

    nice -n 19 python3 -m tools.acceptance.grok_fake --binary ISOLATED_BINARY

Reports bind synthetic results to the binary hash and include cleanup checks.
Authenticated native acceptance is separate from these synthetic checks.

## Fake interface

The shared driver installs the fake as the canonical alias 'grok'. It reports
'grok 1.0.5 (bus-synthetic-fake)' for --version and Grok Build plus the
managed runtime flags for --help. This must remain compatible with the managed provider
identity validation. It never invokes an actual Grok executable.

It reads nested command hooks from the nearest project .grok/hooks/*.json,
in lexical file order, without reading user configuration. All hook payloads
use hookEventName (snake_case values), sessionId, promptId and transcriptPath.
SessionStart uses source:new (source:resume for --resume). Shutdown Stop has
reason:shutdown and no promptId. Nested commands execute with JSON on stdin;
the existing scratch driver supplies its own routing environment.

Synthetic updates.jsonl uses method:_x.ai/session/update and params.sessionId,
params.update.sessionUpdate and params._meta.eventId. Synthetic turn schemas
are provisional and marked synthetic; they do not attest native
completion. chat_history.jsonl and summary.json are companions in
the scratch project's .grok/bus-fake/SESSION directory. No ~/.grok writes.

Scenarios:
- ordinary prompt: fake reply: PROMPT; fake:raw-reply TEXT returns TEXT.
- fake:work N: timed working state with a live editable composer and steering.
- fake:question / fake:card / fake:free-text: inert cards; no displayed command
  executes. Arrow keys choose, Left/Right changes permission scope, Tab parks
  focus, z focuses free text, Enter submits only with card focus.
- fake:gate: provisional Stop before the committed answer.
- fake:gate-cancel: provisional Stop followed by StopCancelled.
- fake:api-error / fake:cancel: failure/cancel never creates successful final.
- fake:long: 40,000 Unicode characters in transcript; hook text clips at 32,768.
- fake:partial: unfinished final JSONL suffix; resolver must remain pending.
- fake:wake N: timer-driven synthetic task completion and a distinct new turn;
  explicitly does not prove native wake. No shell/model is executed.

## Isolated driver commands

Build the joined dev binary once using the resource limits above, then drive a fresh
session through its own target path, e.g.:

    nice -n 19 JOINED_BINARY --dev tui start --name grok-joined-SHA
    nice -n 19 JOINED_BINARY --dev tui status --name grok-joined-SHA
    nice -n 19 JOINED_BINARY --dev tui read --name grok-joined-SHA
    nice -n 19 JOINED_BINARY --dev tui stop --name grok-joined-SHA

Use docs/tui-driver.md and tests/e2e-tui/app.md for exact key/click/CLI syntax.
All authored sends must address that scratch session using its returned
routing values. Never use a default-route Bus CLI, the live binary, live data
root, acceptance_existing_instance, or the real installed Grok alias.

| Journey | Expected observation | Status |
| --- | --- | --- |
| Grok worker / Claude orchestrator | One owned full reply per request; correct room | Record in the hash-bound run report |
| Grok orchestrator / Claude worker | Bootstrap, ordinary authored send (no --async), return idle, durable reply wakes Grok | Record in the hash-bound run report |
| Three queued prompts | Ordered distinct requests/turns/replies, no loss | Record in the hash-bound run report |
| Steering | New message joins intended active turn; same final for both requests | Record in the hash-bound run report |
| Dialog choose / free text | Observe card, answer once with fresh fingerprint; changed scope/parked focus rejects stale input | Record in the hash-bound run report |
| Gate / error / cancel / partial JSONL | No provisional/clipped/error text published as successful final | Record in the hash-bound run report |
| Long final | Exact 40,000-character transcript result, no silent clipping | Record in the hash-bound run report |
| MASTER follow-up while worker runs | Earlier MASTER reply remains immutable; later worker result cannot replace it | Record in the hash-bound run report |
| Work-room wake | One autonomous result in orchestrated room; no unrelated MASTER post | Record in the hash-bound run report |
| Restart before wake | Pending authored work survives; one wake/result, no duplication after repeated reads | Record in the hash-bound run report |
| Adoption / cold resume | Same session, new terminal, owned hooks/capture rebound; missing capture fails | Record in the hash-bound run report |

Record SHA, binary hash, request/session/turn IDs, full message status,
both room histories, diagnostics, screenshots and durable unread counts.
A history map alone cannot establish exactly-once publication. Record unread
deltas or another independent publication counter before/after the wake;
repeat reads/restart and require unchanged publication counts.

## Authenticated acceptance (pending sign-in)

The shared real acceptance harness now delegates Grok-specific cases:

    nice -n 19 python3 -m tools.acceptance.e2e_test --allow-live-models --binary JOINED_BINARY --providers grok,claude --cases single,queued,steering,multi,dialog,orchestrator,adoption,resume

This spends model usage and must run only with separately authorized sign-in.
Its orchestrator case covers MASTER/bootstrap/steering/adoption, not the
worker-wake race checklist above. Native background remains an honest SKIP.
A dialog that the model does not raise is SKIP, not a card-automation PASS.

Native wake status: **PENDING_AUTH**, not waived. A future isolated native PTY
recorder must write the probe's one synthetic prompt exactly once, record all
keyboard writes plus underlying hooks/updates/idle boundaries with monotonic
times, and capture the exact binary version/hash. Require GROK_WAITING to
finish before a genuine idle gap; only after the sleeping task exits with
GROK_WAKE_DONE may a distinct second model turn produce GROK_WOKE. A toast,
same-turn polling, cancellation, extra key input or ambiguous identity fails.

tools.acceptance.grok_probe.run_probe accepts an explicitly supplied native
recorder with authenticated=True and allow_live_models=True. F supplies the
validator, not an unverified real-CLI collector. No recorder is invoked by
default; it returns PENDING_AUTH. The replay CLI:

    nice -n 19 python3 -m tools.acceptance.grok_probe --trace CAPTURE.json

can return REPLAY_VALID, FAIL, TIMEOUT or PENDING_AUTH; replay never returns a
native PASS. Normalized probe events are a tooling contract, not native Grok
fields. Synthetic recorders must mark provenance:synthetic, synthetic:true.
Keep the native recorder/authentication gap visible until authenticated evidence exists.

## Integration ownership

The shared acceptance runner owns Bus start/restart and the Grok cold-resume
adapter. Check original session identity and replacement terminal identity after
restart. Use only the isolated binary and private driver routing values.

Native completion attestation, real trust/permission/question/free-text cards,
long-paste ownership, and idle background reentry require authenticated native
captures. No synthetic result waives these requirements.
