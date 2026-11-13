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

### FEISHU-01 — Optional bridge persists every request route

Preconditions: private fake orchestrators for `feishu-a` and `feishu-b`, plus a
worker in `feishu-a`; recording bridge installed on the fixture PATH.

1. Start the scratch driver and check both role identities. Open Settings and
   require Feishu on, the recipient open_id and the `feishu-bridge path: PATH`
   row. Capture the layout. Close Settings and post untagged progress; require
   no bridge invocation after a quiet window.
2. Post one waiting-tagged request from each orchestrator and two identical
   tagged asks from the worker. Require exactly three independent v1 sends with
   distinct request codes and idempotency keys, retaining all four source IDs:
   the identical worker asks share one DM with a durable alias. The fake must verify
   the actual persisted record and `attempted: true` before returning an ID.
   Check saved returned message/chat IDs, exact raising-agent/orchestrator
   bindings and literal text. Same-agent/same-ask repeats within a fixed thirty
   minutes share the first DM; aliases and its original window survive restart.
   Different agents, asks or recipients retain separate DMs. Only a confirmed DM
   may suppress a repeat; failed or uncertain sends allow a new explicit ask to
   send a new DM. The original attempted send is never retried automatically.
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


## Inbound and real-bridge synthetic acceptance

Run `python3 tools/acceptance/feishu_reply_test.py --binary ABS_BUS --name UNIQUE`
for the combined suite; add `--bridge-binary ABS_REVIEWED_BRIDGE` for the real
bridge's private `listen --input` spool/sink boundary. Importing starts nothing.
Reports and numbered commands remain in `temp/feishu-integration/acceptance/`;
owner-only short runtime roots in `/private/tmp/bfi-*` are retained in the report.
All HOME, settings, Bus identities and processes are private. Providers are the
supplied dev-tools binary's stand-ins; a failing lark sentinel prevents real
transport. Require the supplied identity and both runtime role SHAs/paths to
match HEAD, private permissions, configured recipient/bridge and no legacy CLI key.

1. Untagged/internal-only messages invoke nothing. Four tagged sources produce
   three physical DMs, unique keys/codes and four durable logical routes. The fake
   verifies exact persisted session, source, frozen binding, recipient, literal
   text and attempted key before returning IDs; both worker aliases share IDs.
2. Invoke actual `feishu ingest --instance ROOT --app-id cli_test` with JSON stdin
   and deliberately wrong private inherited instance/agent identity. Only the
   explicit instance receives it. Parent, unknown-parent/root fallback, unique
   code and accepted nested ancestry route exactly. Each original/alias gets its
   own developer prompt, unchanged text, structured source/raising-agent/target/
   external-message provenance, one queue-only recipient, provider start timestamp
   and ended independent turn ID, including while the orchestrator is busy.
3. Unknown, conflicting ancestry or multiple codes park durably without requests.
   Configured authenticated wrong-chat inputs also park; wrong sender/app, bot and
   group inputs fail without consuming an ID. A deleted target never redirects
   to its same-name replacement. Diagnostics stay in logs with no TUI/popup effects.
4. Missing plugin disables sending without off-period replay. Exit-7 sends retain
   attempted uncertainty, log the failure, keep Bus responsive and never retry.
5. Stop only owned scratch roles, prove no leftovers, reopen the same data with
   new PIDs and exact identities, and replay. Original receipts/provider evidence
   remain; edits create no second instruction or send. Cleanup errors fail the run.
6. Optional real bridge uses private config/app/sender/chat and the exact Bus sink.
   Import the flattened event twice: one correlated provider turn and Bus receipt,
   zero sentinel calls. Fake outbound remains isolated. Offline-spool recovery,
   refusal recovery, late parents, registry-marker precedence, sibling acceptance
   order and the immutable F2 workload are separate retained probes. None replaces
   the master-owned live developer reply/replay check.
