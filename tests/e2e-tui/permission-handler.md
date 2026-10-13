# Managed permission handler

## Run contract

Use the e2e-test skill and the branch's `./run tui`, built with
`CARGO_INCREMENTAL=0` and its own target. Run PH-01 through PH-04, each in a
fresh scratch session at **160x45**, default kitty graphics. No SKIP is allowed.
Remove `BUS_DATA_DIR`, `BUS_SESSION_ID` and every `HERDR_*` variable from the
environment of **every** driver invocation. Never drive the live Bus, run a real
provider in this suite, or write private model state to produce outcomes.

The fake Claude service exercises real Bus inspection, fingerprinted choose,
typed decision, audit, routing and clear operations. Its deterministic test
classification does not prove real Claude judgment. Real Claude policy evaluation
is separate. These fixtures never execute the displayed shell commands.
PH-D1's exact live agent5028 incident remains unproven; this suite proves equivalent
native Codex command-screen inspection, including a non-null fingerprint.

## Fixture setup (allowed scratch CLI)

For each case, `T` means `./run tui`, with that case's `--name` on every call.
Start the session and record `status` paths. Use `T bus ...` only for this setup,
the explicit down fixture, and read-only observations below:

```text
T bus room create permission-lab
T bus agent add --room MASTER --name orchestrator --provider claude --pwd WORK --orchestrates permission-lab
T bus agent add --room permission-lab --name worker --provider codex --pwd WORK --consent-hooks
```

`WORK` is this session's scratch work directory (from status/session location),
never a checkout or home outside the session. Remember the created room, worker,
orchestrator and auto-created handler IDs from `T bus state`; do not add the
handler manually. Wait on state predicates, using the handler's actual array
index, until it has a bound `runtime_identity.session_id`, `status=idle`, no
`session_reset_pending` and no reset required. Capture this initial session ID.
Allow setup requests to settle before triggering the worker. Read-only
`T bus state`, `history --room ROOM`, `agent dialog ID`, `agent read ID`,
`message status ID`, `diagnostics`, plus driver screen/effects/waits are allowed.
No other control CLI may approve, route or complete the feature under test.

For UI-trigger actions: click `room:permission-lab`, press `ctrl+p`, capture the
picker, click `recipient:worker`, click `composer`, paste the exact directive,
then press `enter` (individual driver calls). Only the worker receives it.
Use hits/text targets; do not manufacture worker messages with `bus send`.

Capture semantic outputs in the report folder before stopping the session.
The hidden fake service writes **private** `fake-permission-cli.ndjson` in its
scratch cwd (from state). Copy that file read-only before stop: it records actual
inspection JSON, choose result and typed completion, including private full
fingerprints. It is debug evidence, never a room message. Preserve screenshots,
trace, effects and histories; open each asserted screenshot. Always stop the
session, including failures. Do not remove other sessions' scratch paths.

### PH-01 — Safe permission approved and handler cleared

1. Complete fixture setup. Click the work room and open the recipient picker.
   Assert exactly one auto-managed `permission handler` in state, provider Claude,
   room `permission-lab`, bound and idle. Assert no normal sidebar agent row or
   picker recipient for it. Take/open a screenshot and capture hits/picker text.
   Select worker, click composer, paste `fake:permission safe`, press Enter.
2. Wait for the worker's approved decision in state (up to 100s). Capture private
   fixture CLI trace: actual `agent dialog` returned non-null native dialog text
   `Would you like to run the following command?`, the safe cargo test command
   and a fingerprint; actual choose used option1, the same fingerprint, handler
   ID and assignment token and returned closed. Assert exactly one approved
   decision for worker, no escalation/routed/fallback decision, and current
   `agent dialog WORKER` is null. Worker reply has settled; no cargo command ran.
3. Wait until the handler has a **different** bound session ID, is idle, and both
   reset latches/session-reset-pending are false. Assert audit survived clear:
   latest state decision and one work-room approved line name the worker, safe
   summary and digested context. No raw dialog or full `d1.` fingerprint is in
   that line, no approval notice/reply in MASTER, and orchestrator was not woken
   by the approval. Take/open room screenshot; capture both histories/effects.

   Assert room screenshot and work-room history contain no `Permission assignment`
   payload or normal `Permission fixture completed` / `Permission fixture failed`
   handler reply. Required decision audit/notice and worker/orchestrator reply
   remain visible; explicit-ID debug inspection remains available.

### PH-02 — Destructive permission escalated in the work room

1. Complete fixture setup; capture auto-Claude identity and hidden sidebar/picker
   with screenshot/hits. Trigger `fake:permission destructive` through worker
   recipient, composer paste and Enter exactly as above.
2. Wait for one escalated decision for worker. Capture actual native `agent dialog`
   with the outside-worker deletion command and fingerprint. Private fixture CLI
   trace must contain inspection and typed escalated completion, **no choose or
   answer**. Assert the dialog remains open, no command executed, no approved
   outcome and no fallback. Capture state and work-room history.
3. Wait for orchestrator's reply in `permission-lab`. Assert the single escalation
   names worker/ID, outside-scope reason, episode and fingerprint digest in one
   line; it excludes raw command/dialog and full `d1.` context. Assert no matching
   escalation or orchestrator reply in MASTER. This is PR50 worker-woken room
   routing. Take/open screenshot; capture both histories and effects.

   Assert room screenshot and work-room history contain no `Permission assignment`
   payload or normal `Permission fixture completed` / `Permission fixture failed`
   handler reply. Required decision audit/notice and worker/orchestrator reply
   remain visible; explicit-ID debug inspection remains available.

### PH-03 — Question routed without answering

1. Complete fixture setup; capture auto-Claude identity and hidden sidebar/picker
   with screenshot/hits. Trigger `fake:permission question` through worker
   recipient, composer paste and Enter exactly as above.
2. Wait for one routed decision for worker. Capture actual native question
   `Which design should the worker use?` with Alpha/Beta and fingerprint. Private
   fixture CLI trace must contain inspection and typed routed completion, **no
   choose or answer**. Assert question remains open, no approved/escalated/fallback
   outcome. Capture state and work-room history.
3. Wait for orchestrator's work-room reply. Assert the one-line notice names
   worker/ID, explicitly says not permission-related and requires orchestrator
   choice, with digested episode/fingerprint context. It excludes raw question,
   options and full `d1.` context. Assert no matching notice/reply in MASTER.
   Take/open screenshot; capture both histories and effects.

   Assert room screenshot and work-room history contain no `Permission assignment`
   payload or normal `Permission fixture completed` / `Permission fixture failed`
   handler reply. Required decision audit/notice and worker/orchestrator reply
   remain visible; explicit-ID debug inspection remains available.

### PH-04 — Handler unavailable falls back once

1. Complete fixture setup; capture auto-Claude identity and hidden sidebar/picker
   with screenshot/hits. As explicit-ID debug **setup only**, run
   `T bus send --room permission-lab --to HANDLER_ID --text fake:exit`.
   Wait until handler is unavailable (not ready/native process gone; corroborate
   state/diagnostics), retaining the same managed ID. Do not delete/replace it.
2. Trigger `fake:permission safe` through worker recipient, composer paste and
   Enter. Wait for one fallback decision for worker, with no approved decision.
   Assert actual native command dialog remains open. The unavailable handler's
   fixture CLI trace contains no choose, answer or completion for this episode.
   Capture state, diagnostic reason and work-room history.
3. Wait for orchestrator's work-room reply. Assert exactly one concise fallback
   notice names worker, unavailable reason and digested episode context; no raw
   dialog/full fingerprint and no matching notice/reply in MASTER. Observe the
   settled room again using state/history (no duplicate notice or new assignment
   for the same dialog). Take/open screenshot; capture both histories/effects.
   Assert room screenshot and work-room history contain no `Permission assignment`
   payload or normal `Permission fixture completed` / `Permission fixture failed`
   handler reply. Required decision audit/notice and worker/orchestrator reply
   remain visible; explicit-ID debug inspection remains available.
