# Blocked-dialog delivery during an orchestrator turn

## Run contract

Use one fresh scratch Bus through the dev TUI driver at 120x40 with inherited
`BUS_*` and `HERDR_*` removed. Use fake providers and a closed PATH with a
recording `lark-cli`; never use the live Bus, real providers or Feishu. No SKIP
is allowed. Capture a cell screenshot and open it.

## Setup

Through `T bus --name NAME`, create `blocked-notice-lab`, add a Cursor MASTER
orchestrator `orch` bound to it and a Codex worker `worker`. Both use the private
WORK directory with hook consent. Wait until idle. CLI sends may establish the
active turn and ordinary backlog; submit the question through the room UI.
Read-only state/history and observe/press may corroborate the journey.

### BN-01 — A text question reaches the active orchestrator past a cross-room backlog

1. Send `fake:work 180` to `orch` in `blocked-notice-lab`. Wait for its request
   to be active with a trusted start and the orchestrator working. Send
   `ordinary first` and `ordinary second` to `orch` in MASTER. Both remain
   queued in that order while the original work-room request is active.
2. Click `room:blocked-notice-lab`, press `ctrl+p`, click `recipient:worker`,
   press `esc`, click `composer`, type `fake:question-text`, then press `enter`,
   one action per call. Wait for its Blocked notice to join the orchestrator's
   active turn. Both ordinary MASTER messages remain queued in order and
   exactly one Blocked notice exists. `bus agent observe worker` must show the
   Chinese question asking for the bug's record id or row link and its submit
   controls. Click `agent:worker`, then capture/open a native cell screenshot.
3. Observe again and run `bus agent press worker --text record-fixture --submit
   --terminal-id TERMINAL_ID`. Require outcome `sent`, then prove acceptance by
   waiting for the worker's question request to reply and observing its ordinary
   composer. History retains exactly one Blocked notice and adds no Resolved
   notice. The ordinary MASTER requests remain queued in their original order.
   Stop the scratch session.
