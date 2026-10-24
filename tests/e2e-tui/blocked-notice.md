# Blocked-dialog delivery during an orchestrator turn

## Run contract

Use one fresh scratch Bus through the dev TUI driver at 120x40. Remove inherited
`BUS_*` and `HERDR_*`. Use fake providers and a closed PATH with a recording
`lark-cli`; never use the live Bus, project hooks, real providers or Feishu.
No SKIP is allowed. Capture a cell screenshot and open it.

## Setup

CLI fixture setup is allowed through `T bus --name NAME`: create
`blocked-notice-lab`, add a Cursor MASTER orchestrator named `orch` bound to it,
and add a Codex worker named `worker`. Both use the session's private WORK
directory with hook consent. Cursor's live composer lets the fake accept input
during a running turn. Wait until both are idle. CLI sends may establish the
active turn and ordinary backlog below. After setup, submit the question through
the room UI. Read-only state/history and dialog/answer APIs may corroborate it.

### BN-01 — A free-text question reaches the active orchestrator past a cross-room backlog

1. Send `fake:work 180` to `orch` in `blocked-notice-lab`. Wait for its request
   to be active with a trusted start and the orchestrator working. Send
   `ordinary first` and `ordinary second` to `orch` in MASTER. Verify that both
   remain queued in that order while the original work-room request is active.
2. Click `room:blocked-notice-lab`, press `ctrl+p`, click `recipient:worker`,
   press `esc`, click `composer`, type `fake:question-text`, then press `enter`,
   one driver call per action. Wait for its Blocked notice to be submitted into
   the orchestrator's active turn. Verify that the notice is active and joined
   to that original request, both ordinary MASTER messages remain queued in
   their original order, and exactly one blocked notice exists. Observe
   `bus agent dialog worker`: kind `question`, options `[]`, a fingerprint, and
   the Chinese question asking for the bug's record id or row link. Click
   `agent:worker`, capture and open a screenshot showing the native question.
3. Fetch a fresh dialog fingerprint and answer with `bus agent answer worker
   --text record-fixture --fingerprint F`. Require outcome `closed`, then wait
   for the worker's question request to reply. A fresh dialog read is null and
   history contains one Blocked and one Resolved notice. The ordinary MASTER
   requests remain queued in their original order. Stop the scratch session.
