# Worker messages to its orchestrator

Run only on a fresh scratch Bus through the TUI driver, at 140x40, with fake
providers. No desktop automation or real model use is needed. CLI commands below
are agent-authored message fixtures and read-only evidence; room and agent setup
also uses the scratch CLI. Record receipts, message status, history, terminal
reads and screen text. No steps may be skipped.

## Cases (run order)

### WOC-01 — Unprompted work-room messages reach only its bound MASTER orchestrator

**Preconditions/setup:** Create `qa-work` and `qa-other` with the scratch CLI.
Add `qa-worker` (Codex) to `qa-work`. Add `libtv-app master` (Claude) in MASTER,
bound to `qa-work`, and `other-orch` (Claude) in MASTER, bound to `qa-other`.
Use only the session's scratch work directory for PWD, consenting to hook setup.
Wait for all three fake agents to become idle. Record room/agent IDs. Confirm
`qa-work` history is empty: no orchestrator has initiated a worker turn.

1. Fixture: send `WOC alias blocker` from `qa-worker` in `qa-work` to
   `Orchestrator`. Wait for its message to settle. Click `room:qa-work` and
   inspect history and the orchestrator's terminal → one normal worker-authored
   work-room message addressed to the bound orchestrator, delivered into its
   session with the `qa-worker (agent ID) in room ROOM_ID:` attribution header,
   with a `fake reply:` answering that text; both message and reply
   show in the work-room chat. MASTER history remains empty.
2. Fixture: send `WOC name ready` the same way, targeting `libtv-app master` by
   name, then send `WOC id question` targeting its numeric ID. Wait for both
   messages to settle → both use the same orchestrator agent/session and each
   has its own normal reply in `qa-work` history.
3. Fixture: send `fake:work 8 WOC MASTER busy` as the developer in MASTER to
   the bound orchestrator. Wait until it is working, then send `WOC queued verify`
   from the worker in `qa-work` to `orchestrator`, without `--queue`. Inspect its
   status while busy, then wait for both to settle → the worker message stays
   queued until the MASTER turn ends and receives its own attributed reply;
   the MASTER reply contains only the MASTER request, and neither reply is
   substituted for the other.
4. Fixture: send from `qa-worker` to `other-orch` by name and ID in `qa-work`;
   also send without `--as` to `orchestrator` in a newly created room with no orchestrator →
   the first two fail with `No matching room or agent`; the last fails with
   `This room has no bound orchestrator`. No failed send adds a history row or
   a request, and `other-orch` receives no message.
5. In `qa-work`, press Ctrl+P → the picker lists `qa-worker` and `libtv-app
   master`, and excludes `other-orch`. Click All → only `qa-worker` is checked,
   All shows checked, and the orchestrator remains unchecked. Click All again →
   the worker clears. Select `recipient:libtv-app master`,
   close the picker with Esc, click the composer, type `WOC picker verify` and
   press Enter. Wait for its reply → the developer-authored message and its
   orchestrator reply appear in `qa-work` history, proving selection persists
   and submits to the orchestrator.

**Evidence:** CLI receipts and status/history JSON, orchestrator terminal text,
picker and chat snapshots. The driver trace records every action; no real
desktop screenshot is required.
