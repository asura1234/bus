# Feishu waiting-on-you bot DMs

## Run contract

Use fresh scratch Bus sessions with the own-worktree dev-tools binary, 160x55,
graphics none. No desktop automation, live Bus, real providers or real Feishu sends.
Run `tools/acceptance/feishu_notify_test.py` with a recording lark-cli as the only
CLI on a closed PATH plus system directories. The script records complete argv.
CLI fixtures may create a fake MASTER orchestrator and post `bus send --to human`;
all settings edits and toggles use real driver UI input. Read-only state, settings,
CLI ledger, effects and logs corroborate results. No skips or failed-step retries.
Quiet windows use driver `wait --stable 3s --rows 0-0 --timeout 8s`.
Open and inspect the cell screenshots before accepting the report.

### FEISHU-01 — Waiting tags deliver through the bot without terminal popups

Preconditions: fresh scratch Bus, fake MASTER orchestrator notify-orch bound to notify-work,
recording lark-cli, default recipient ou_ca1b530d792419a044e281f998ce68dc.

1. Open Settings. Assert Feishu notifications and Freeze recovery notifications both default on, recipient and PATH rows visible, and no Developer message notifications toggle. Capture and inspect settings screenshot. Close Settings, select notify-work and type KEEP DRAFT.
2. Post untagged progress and `[needs_developer_attention]` alone; after a quiet window assert zero CLI calls. Post `[waiting_on_you]` followed by a line `Approve access` and a second private line through MASTER --to human. Assert exactly one bot DM invocation, correct recipient and exact plain action text, with no second private line or added attachment paths. After a quiet window assert no terminal notification effects, no duplicate send and KEEP DRAFT unchanged.
3. Post `[waiting-on-you] First update`, untagged progress, and `WAITING ON YOU: Second update` within two seconds. Assert one additional invocation containing `notify-orch is waiting on you: First update and 1 more`, then no duplicate after a quiet window. Every recorded invocation must be im +messages-send with --as bot, --user-id and --text, with no chat-id or attachments.
4. Open Settings and toggle Feishu off. Assert persisted false and freeze still true; capture and inspect screenshot. Post a waiting tag while off and assert no send. Toggle on through UI and assert disabled history stays quiet.
5. Edit Recipient open_id through UI to ou_fake_recipient and lark-cli path to the recording shim; assert settings persisted. Post `still waiting on you: Configured action` and assert one additional DM to ou_fake_recipient. Capture and inspect edited settings screenshot.
6. Make the shim fail with exit 7; post one waiting tag. Assert one recorded invocation, a bus.feishu.failed warning with error, responsive scratch state and no retry through a quiet window. Confirm no developer terminal popup and stop the scratch session.
