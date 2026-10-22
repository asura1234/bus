# Grok provider acceptance: synthetic fixtures and pending native evidence

Lane F baseline: G0 5eee9b6f1b4a257f42dcc5c4f25a27f326845aa7.
Only the six F-owned files change. Shared registration and integrated journeys
belong to grok-scout (G). This checklist does not record a journey PASS.

All fake turns, final-boundary records, permission/question cards and wakes
below are **SYNTHETIC**. The observed 1.0.5 evidence establishes camelCase
session/shutdown hooks and the ACP update envelope only. It does not establish
native final markers, authenticated cards, or automatic background reentry.
B/C fixture alignment and authenticated acceptance remain required.

## Local verification

Scrub inherited BUS_* / HERDR_* before every command. Run from the F worktree,
with these process-scoped settings:

    CARGO_TARGET_DIR=/Users/dylanliu/work/bus-targets/grok-f
    CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1
    RUST_TEST_THREADS=1

    nice -n 19 cargo test --locked --bin bus --features dev-tools grok_f_ -- --test-threads=1
    nice -n 19 python3 -m unittest tools.tests.grok_e2e_test tools.tests.grok_probe_test

No full gate, real provider, Feishu message or live Bus is part of these tests.
Delete only the lane's CARGO_TARGET_DIR when verification is finished.

## Fake interface

The shared driver installs the fake as the canonical alias 'grok'. It reports
'grok 1.0.5 (bus-synthetic-fake)' for --version and Grok Build plus the
managed runtime flags for --help. This must remain compatible with A+D's
identity validation. It never invokes an actual Grok executable.

It reads nested command hooks from the nearest project .grok/hooks/*.json,
in lexical file order, without reading user configuration. All hook payloads
use hookEventName (snake_case values), sessionId, promptId and transcriptPath.
SessionStart uses source:new (source:resume for --resume). Shutdown Stop has
reason:shutdown and no promptId. Nested commands execute with JSON on stdin;
the existing scratch driver supplies its own routing environment.

Synthetic updates.jsonl uses method:_x.ai/session/update and params.sessionId,
params.update.sessionUpdate and params._meta.eventId. Synthetic turn schemas
are provisional, marked synthetic, and require B's agreement before joined
completion acceptance. chat_history.jsonl and summary.json are companions in
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

## Post-join commands (G owns execution and exact joined SHA)

Build the joined dev binary once using G's resource limits, then drive a fresh
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
| Grok worker / Claude orchestrator | One owned full reply per request; correct room | UNRUN: joined A/B/C/E required |
| Grok orchestrator / Claude worker | Bootstrap, ordinary authored send (no --async), return idle, durable reply wakes Grok | UNRUN: joined code |
| Three queued prompts | Ordered distinct requests/turns/replies, no loss | UNRUN: joined code |
| Steering | New message joins intended active turn; same final for both requests | UNRUN: joined code |
| Dialog choose / free text | Observe card, answer once with fresh fingerprint; changed scope/parked focus rejects stale input | UNRUN: joined C |
| Gate / error / cancel / partial JSONL | No provisional/clipped/error text published as successful final | UNRUN: joined B |
| Long final | Exact 40,000-character transcript result, no silent clipping | UNRUN: joined B |
| MASTER follow-up while worker runs | Earlier MASTER reply remains immutable; later worker result cannot replace it | UNRUN: joined E/G |
| Work-room wake | One autonomous result in orchestrated room; no unrelated MASTER post | UNRUN: joined E/G |
| Restart before wake | Pending authored work survives; one wake/result, no duplication after repeated reads | UNRUN: joined E/G |
| Adoption / cold resume | Same session, new terminal, owned hooks/capture rebound; missing capture fails | UNRUN: joined A/G |

Record SHA, binary hash, request/session/turn IDs, full message status,
both room histories, diagnostics, screenshots and durable unread counts.
A history map alone cannot establish exactly-once publication. Record unread
deltas or another independent publication counter before/after the wake;
repeat reads/restart and require unchanged publication counts.

## Authenticated acceptance (not executed by lane F)

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
Keep the native recorder/authentication gap visible until G captures evidence.

## Integration notes

The shared Run.run_case replaces the ordinary resume callback for Grok. F's
adapter repeats the original-session and new-terminal checks using report.resume;
G may instead preserve the generic callback in shared code after integration.
The F runner never independently starts/restarts Bus outside that shared Run.

Attempted coordination: the required 'bus send --room 568 --to grok-scout'
could not run because bus was absent from the scrubbed PATH (exit 127).
G must align A+D identity and B/C transcript/card contracts before claiming
joined fake success. No live installation was used to work around that gap.

## Lane F verification record

- Focused Rust: 13 F tests passed; G0 registration/invalid-directory checks 2 passed.
- Python acceptance/probe: 25 tests passed. All inputs are synthetic.
- Standalone synthetic fake PTY smoke: version identity, nested hook execution,
  ordinary reply, coalesced free-text card answer, shutdown without promptId,
  same-session resume, and three unique prompt IDs passed. No Bus server ran.
- The review's fake-version/native-PASS finding was reproduced RED and fixed;
  native identity now requires a real hexadecimal build suffix.
- No full gate or joined scratch Bus journey was run. Authenticated model,
  real cards, authoritative final boundaries and native wake remain pending.
