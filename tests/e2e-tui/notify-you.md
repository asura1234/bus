# Developer addressing and mandatory notifications

## Run contract

Fresh scratch Bus through the own-worktree dev-tools driver, 160x55, graphics none.
Never use desktop automation, real provider processes, real Feishu or audio playback.
Start with a closed PATH containing a recording lark-cli shim and system tools only;
the driver supplies fake providers and recorded sound effects. CLI fixtures may create
rooms, fake agents and agent-authored sends, and set legacy Feishu off or room sounds
off as setup. All composer input and room/focus changes use driver UI. Read-only history,
state, effect ledger, settings and recording-shim argv corroborate the assertions.
Quiet windows use `wait --stable 3s --rows 0-0 --timeout 8s`. No skips or failed-step retries.
Cell screenshots may be captured; no real-desktop screenshots are permitted.

### YOU-01 — Any agent reaches the developer from any room and always rings

Preconditions: fake MASTER orchestrator `notify-orch` bound to `notify-work`, fake worker
`notify-worker` in that work room; second work room `notify-other`. All room sounds off,
legacy `feishu_waiting_notifications` false; recording bot CLI configured. Wait until fake
agents are idle and setup activity has settled before recording baseline effect counts.

1. Open Settings and assert Feishu notifications says always on, with editable recipient and CLI path, and room sounds off. Try Enter on the Feishu row and assert it stays on. Close Settings, select notify-other and type KEEP DRAFT. Save sound and shim-call baselines.
2. Fixture: post `Decide release timing` as notify-worker to `you` in notify-work. Assert posted with no requests; work-room history attributes it to notify-worker with empty recipients. Wait for one new recorded sound effect and one bot DM containing `notify-worker to you: Decide release timing`; no terminal notification. The visible room and KEEP DRAFT stay unchanged. After a quiet window assert no repeat.
3. Select notify-work and assert the worker message displays with a You recipient. Fixture: post `[waiting-on-you] Approve access` as notify-orch to you in notify-work, then `MASTER decision` as notify-worker to you in MASTER. Assert each remains in the selected send room, with the correct author, and exactly two new recorded sound effects despite arriving within the normal cooldown. Assert both actions appear in recorded bot DMs.
4. Select notify-other and send focus out. Fixture: post a tagged peer message `[waiting-on-you] Choose rollout` from notify-worker to notify-orch in notify-work. Assert a new mandatory sound and bot DM despite off room sounds and focus out. After its fake reply settles, record baselines. Post an untagged peer message and a developer-authored `[waiting-on-you]` prompt (use `fake:raw-reply done` so the reply has no tag). Assert neither creates mandatory sound or a bot DM through the quiet window. KEEP DRAFT and conversation room remain unchanged.
5. Fixture: send to human from notify-worker in notify-other; assert alias posts there and notifies. Attempt mixed you/agent recipients, you with queue, you with async, and you without as; assert explicit failures with no new alerts. Attempt CLI Feishu off; assert rejection. Assert recorded sounds use a different file from an ordinary room notification (enable a room sound for an untagged peer-send fixture and compare recorded targets), and the bot argv uses --as bot, --user-id and --text. Stop the scratch session.
