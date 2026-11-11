# Open agent messaging

Run only on a fresh scratch Bus through the TUI driver, at 140x40, with fake
providers. No desktop automation or real model use is needed. CLI commands below
are agent-authored message fixtures and read-only evidence; room and agent setup
also uses the scratch CLI. Record receipts, message status, history, terminal
reads and screen text. No steps may be skipped. Ordinary fake native finals
settle requests with `complete:true`, `stage:ended`, `reply:null` and no failure
reason; their exact output is native evidence, never an invented chat reply.

## Cases (run order)

### WOC-01 — Agent messages reach peers across rooms with sender attribution

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
   session with the `qa-worker (room ROOM_ID):` attribution header,
   with matching `fake reply:` output in the native pane. Only the authored
   message shows in work-room chat; its request meets the ended/null contract above. MASTER history remains empty.
2. Fixture: send `WOC name ready` the same way, targeting `libtv-app master` by
   name, then send `WOC id question` targeting its numeric ID. Wait for both
   messages to settle → both use the same orchestrator agent/session and each
   has its own ended request in `qa-work` history and matching native output, with no stored reply.
3. Fixture: send `fake:work 8 WOC MASTER busy` as the developer in MASTER to
   the bound orchestrator. Wait until it is working, then send `WOC queued verify`
   from the worker in `qa-work` to `orchestrator`, without `--queue`. Inspect its
   status while busy, then wait for both to settle → the worker message stays
   queued until the MASTER turn ends and then gets its own attributed turn;
   both requests settle ended/null. The two original prompts stay in their own
   conversations and each has matching native output, with no cross-attributed reply.
4. Fixture: send from `qa-worker` to `other-orch` by name and ID in `qa-work` →
   both reach that unrelated MASTER orchestrator, settle as distinct ended
   requests in `qa-work`, and carry the worker's attribution. Send without `--as` to
   `orchestrator` in a newly created unbound room → clear missing-binding error,
   no history row. Add an agent named `duplicate` to MASTER (bound to a new `qa-duplicate` room)
   then try to add the same name in `qa-other` → the add is rejected with the
   existing owner ID and MASTER room; no second agent is created. Send to
   `duplicate` from `qa-worker` → the unique existing owner receives the message
   globally in the `qa-work` conversation.
5. In `qa-work`, press Ctrl+P → the picker lists `qa-worker` and `libtv-app
   master`, and excludes `other-orch`. Click All → only `qa-worker` is checked,
   All shows checked, and the orchestrator remains unchecked. Click All again →
   the worker clears. Select `recipient:libtv-app master`,
   close the picker with Esc, click the composer, type `WOC picker verify` and
   press Enter. Wait for native completion → the developer-authored message
   appears in `qa-work` history once, with an ended/null request and matching
   native output, proving selection persists and submits to the orchestrator.

6. Fixture: send `qa-worker here —` followed by a newline and `WOC already
   identified` to `orchestrator` as `qa-worker` → the delivered payload and its native echo carry one canonical `qa-worker (room ROOM_ID):` header followed
   by `WOC already identified`, without the legacy identity line.

**Evidence:** CLI receipts and status/history JSON, orchestrator terminal text,
picker and chat snapshots. The driver trace records every action; no real
desktop screenshot is required.
