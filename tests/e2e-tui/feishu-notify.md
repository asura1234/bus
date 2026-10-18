# Self-only Feishu notifications

## Run contract

Use a scratch Bus and fake providers only, 160x55, graphics none. Never drive
the live Bus and never send real Feishu. Run `tools/acceptance/feishu_notify_test.py`
with the own-worktree dev-tools binary. Its recording lark-cli is the only CLI
on a closed PATH plus system directories. Every invocation is recorded; every
send must name exactly the authenticated self ID, use `--as user`, have no
chat recipient and start with `[Bus]` plus machine/session/room/agent context.
Public CLI commands are allowed for setup, explicit CLI toggle steps and
posting fixtures. UI toggles and visible badge assertions use real driver input.
No skips are allowed.

### FEISHU-01 — Runtime self-only notifications

**Preconditions/setup:** scratch room `notify-work`, MASTER fake Claude
orchestrator `notify-orch`, recording lark-cli with configurable auth fixture.

1. Open Settings. Assert the Feishu checkbox starts off. In missing-auth mode click it; wait for refusal, visible reason and persisted off. With fixture CLI absent, CLI enable also refuses without fallback.
2. Restore ready auth and click the checkbox. Wait for effective on, capture screenshot and settings. CLI off then CLI on take effect in the same process. Verify no sends during setup and reject all destination parameters.
3. Post untagged progress and a tagged orchestrator MASTER message. Only tagged text is sent, all context and self-only recipient assertions pass, MASTER renders the badge without the raw prefix. Capture screenshot.
4. Post the same tagged content again; verify it is consumed without another send. Toggle off, post a tag and verify no send; enable again and verify disabled history is skipped.
5. Send the orchestrator `fake:raw-reply [needs_developer_attention] Final decision`. The final forwards once and displays the badge. Send `fake:dialog 20`; its mechanical Bus notice forwards once, identifies the blocked orchestrator, and duplicates do not send again. Assert every recorded send's recipient is self and all bodies are attributed to Bus.
6. Post five distinct tagged decisions as a burst. Verify total sends stop at six in the current rolling minute; remaining sources stay in MASTER history. Check every send's self-only recipient and Bus attribution again.

**Evidence:** driver traces, recorded CLI JSONL, state/history, styled cells and opened screenshots, process ID continuity, report bound to candidate commit/tree.

**Pass criteria:** every step passes, scratch stopped, no real outbound message.
