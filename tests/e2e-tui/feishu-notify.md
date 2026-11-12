# Feishu bridge request and reply acceptance

## Run contract

This is an opt-in private acceptance suite. Importing the Python module, including
pytest collection, starts no Bus, provider, bridge or Feishu transport.

Use the current-head author-worktree dev-tools binary:

```sh
python3 tools/acceptance/feishu_notify_test.py \
  --binary /absolute/private-target/debug/bus \
  --name feishu-bridge-final
```

The script requires `--binary` to be an absolute executable and checks its
`--identity` SHA against the checkout HEAD. It also checks the running scratch
client and native server paths/SHAs. It does not build anything.

Reports and numbered control/ingest evidence are written beneath
`temp/feishu-integration/acceptance/<name>-<unique>/`. A fresh owner-only runtime
root `/private/tmp/bfi-<unique>` is recorded and retained; Darwin's socket paths
cannot fit beneath the longer worktree artifact path. Every Bus control command,
provider, settings file, HOME, effect recorder and bridge belongs to that private
runtime. Inherited BUS/HERDR and provider configuration are replaced with a closed
fixture environment. The suite never restarts, signals or queries a live Bus.

`claude` and `codex` on the fixture PATH are the supplied Bus binary's dev-tools
provider stand-ins. `feishu-bridge` is a recording Python fixture. `lark-cli` is a
failing sentinel, so a transport call fails the suite. No real provider account,
Feishu credentials, outbound Feishu send or developer interaction is used.

A failure exits nonzero and leaves `report.json`, `report.md`, `failure.txt` and
all command evidence. A pass proves only the evidence classes listed below; it
does not satisfy the final real developer-reply acceptance hold.

### FEISHU-01 — Exact build and optional-plugin isolation

1. Require the supplied binary's SHA and both scratch runtime role SHAs to equal
   current HEAD; require all role executable paths to match the supplied binary.
2. Check the private HOME/data permissions and fake-provider PATH. Start fake
   orchestrators `orch-a` and `orch-b` bound to separate work rooms, plus
   `worker-a` in `work-a`.
3. Require settings to expose `feishu_bridge_path` and the configured recipient.
   The old `feishu_cli_path` key must be absent. The fake installed bridge enables
   Feishu; no real bridge or lark transport is selected.

### FEISHU-02 — One persisted outbound record per Bus request

1. Post untagged progress and `[needs_developer_attention]` text. After a quiet
   window, require zero bridge invocations.
2. Post separate waiting tags from both orchestrators, then two identical tagged
   asks from `worker-a`. Require exactly four bridge invocations, four route
   codes, four stable idempotency keys and four distinct source prompt IDs.
3. Inside each fake bridge invocation, read the actual scratch `state.json`.
   Require exactly one matching persisted outbox record, `attempted: true`, the
   correct session/source/raising-agent/orchestrator/recipient, and the exact text
   and idempotency key passed through v1 `send --open-id ... --text ...
   --idempotency-key ...`. The fake returns a unique `om_...` and `oc_dm`.
4. Require Bus to persist all four returned IDs. A second request's text must not
   be combined into the first DM. Verify both identical worker asks retain their
   own source mappings to `orch-a`.

### FEISHU-03 — Authenticated replies produce independent provider turns

1. Invoke the actual standalone command with JSON stdin:
   `bus feishu ingest --instance <scratch-data> --app-id cli_test`. Supply a
   deliberately wrong private inherited BUS_DATA_DIR and worker caller identity;
   only the explicit instance may receive the message.
2. Reply to the first outbound DM with a bounded fake-provider work directive.
   Once its provider start is bound, submit a worker-request reply to the same
   orchestrator. Require two independent turns with distinct provider turn IDs.
3. For each delivery, require a developer (`human`) prompt, unchanged reply text,
   structured `in_reply_to` source/raising-agent/orchestrator/external-message
   provenance, one recipient, `queue: true`, `start_bound: true`, a nonnull
   `provider_started_at_ms`, and an ended fake-provider turn.
4. Replay the original event and replay its external ID with changed text.
   Require the same durable receipt and one original request.
5. Exercise unknown direct-parent/root fallback, a standalone short request code,
   and a nested reply to a previously ingested message. Require delivery only to
   each recorded orchestrator, with one request per external message ID.

### FEISHU-04 — Unknown, ambiguous and unauthorized replies stay undelivered

1. Submit an unmapped reply, conflicting parent/root mappings, and two conflicting
   request codes. Require durable parked receipts, retained inbox records and no
   provider requests for those external IDs.
2. Submit another sender, bot sender, group chat, wrong mapped chat and wrong app.
   Require nonzero sink exits, no inbox consumption and no provider delivery.
3. Delete `orch-b`, then create a replacement with its old name. Reply to the old
   outbound mapping. Require parking rather than delivery to the replacement.
4. Require logged parking diagnostics and no desktop/terminal notification
   effects or Feishu error text in the scratch TUI.

### FEISHU-05 — Missing plugin is off; send errors stay in logs

1. Point the scratch bridge setting at a missing executable. Require disabled
   Feishu and no send for a new waiting tag. Restore the fake plugin; the
   off-period alert must not replay.
2. Make the fake bridge fail with exit 7 and post a fresh waiting tag. Require
   one attempted invocation, `bus.feishu.failed` with the exit category in the
   private logs, a responsive Bus, and no automatic resend or error popup.
3. Retain the failed attempted outbox record for uncertainty audit. Returning the
   fixture to success must not silently resend it.

### FEISHU-06 — Restart retains exactly-once receipts and attempted sends

1. Save the original accepted request and bridge invocation count. Stop only the
   private scratch client and native server; require no owned process leftovers.
2. Start the same scratch data root with the same binary. Require different
   client/native-server PIDs and exact-head identity again.
3. Replay the original external event. Require the original receipt/request,
   unchanged provider-turn evidence and no additional outbound invocation.
4. Stop the private scratch session and retain its runtime/evidence paths. Any
   cleanup error or owned process leftover makes the report fail.

## Optional real bridge, synthetic input only

After the separate bridge implementation is reviewed, the same suite can also
exercise its real spool/sink boundary:

```sh
python3 tools/acceptance/feishu_notify_test.py \
  --binary /absolute/private-target/debug/bus \
  --bridge-binary /absolute/feishu-bridge \
  --name feishu-bridge-synthetic
```

The script generates a private config/state directory with the test app, sender,
chat and exact scratch Bus sink. It runs only `listen --input` against one
flattened synthetic text event, then imports it again. Require one correlated
fake-provider turn, a single Bus receipt and no invocation of the failing
`lark-cli` sentinel. Bus outbound still uses the recording fake bridge. This is
real bridge integration evidence without real Feishu transport or a live reply.
