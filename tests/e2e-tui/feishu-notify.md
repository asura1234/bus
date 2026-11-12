# Feishu outbound notifications

## Run contract

Use the author-worktree's current-head dev-tools binary with the opt-in driver:

```sh
python3 tools/acceptance/feishu_notify_test.py \
  --binary /absolute/private-target/debug/bus --name feishu-outbound
```

Importing the module starts nothing. The script uses `notification_driver.py`,
its own report document and the existing report renderer. It creates a fresh
private driver session with fake providers, effect recorders and a closed PATH
containing a recording `feishu-bridge`. The fixture reads the actual scratch
`BUS_DATA_DIR/state.json` to verify the outbox before sending, then returns unique
v1 message/chat IDs. It does not use credentials, real providers, the live Bus,
desktop automation or real Feishu traffic.

Require both scratch runtime role SHAs and executable paths to identify the
current-head supplied binary. Driver traces, screenshots, numbered calls,
recorded bridge entries and report output are retained under
`temp/notification-driver/`. Quiet windows use
`wait --stable 3s --rows 0-0 --timeout 8s`. No skipped steps or failed-step retries.
This procedure proves outbound behavior only.

### FEISHU-01 — Optional bridge sends separate durable request notifications

Preconditions: private fake orchestrators for `feishu-a` and `feishu-b`, plus a
worker in `feishu-a`; recording bridge installed on the fixture PATH.

1. Start the scratch driver and check both role identities. Open Settings and
   require Feishu on, the recipient open_id and the `feishu-bridge path: PATH`
   row. Capture the layout. Close Settings and post untagged progress; require
   no bridge invocation after a quiet window.
2. Post one waiting-tagged request from each orchestrator and two identical
   tagged asks from the worker. Require exactly four independent v1 sends with
   distinct source IDs, request codes and idempotency keys. The fake must verify
   the actual persisted record and `attempted: true` before returning an ID.
   Check saved returned message/chat IDs, exact raising-agent/orchestrator
   bindings and literal text, with no cross-request batching or duplicate send.
3. Through Settings, select a missing explicit bridge path. Require Feishu off
   and no send for a new waiting tag. Restore PATH discovery and require Feishu
   on without replaying the off-period request.
4. Make the fixture exit 7, then post a fresh waiting tag. Require one attempted
   invocation, an uncertain outbox record without a returned message ID,
   `bus.feishu.failed` in the private log and no retry. Confirm Bus remains
   responsive, the error is absent from its TUI, and there is no terminal popup.
5. Restore the fixture and use Settings to choose its explicit path and another
   recipient open_id. Post a new waiting tag; require exactly one mapped DM to
   that recipient. Verify the failed attempt is not replayed. Retain the full
   bridge ledger and stop only this private driver session.
