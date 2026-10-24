# Developer notifications

Any worker or orchestrator may address the developer in any room with
`bus send --room ROOM --as AGENT --to you --text TEXT`. `human` remains an alias.
The agent-authored message stays in that conversation room, displays **You** as
recipient, and creates no agent request. When and how to write to the developer is in the
[Bus communication guide](../orchestration/bus-communication-guide.md).

There are two routes. The **TUI alert** (unread dot and developer-alert sound) is for the
developer at the computer. The **Feishu DM** pages the developer anywhere, so it carries only
`[waiting-on-you]`/`[waiting_on_you]`-tagged hard blockers.

Every direct agent message to you raises the TUI alert, including untagged messages. Agent-authored
peer messages and completed agent replies also notify when they contain an explicit `@you`,
`[waiting-on-you]` or `[waiting_on_you]` marker (case-insensitive). The legacy whole phrase
`waiting on you`, including `still waiting on you`, raises the TUI alert only in direct developer notices
and replies to developer-authored requests. Bare phrases between agents do not trigger developer alerts;
there, "you" names the receiving agent. Developer-authored sends never trigger alerts.

When the You sound checkbox is enabled, the selected system sound rings once for each
snapshot that brings one or more qualifying sources, regardless of visible room, focus, room sound switches, startup grace
or ordinary notification cooldown.
Only the coordinator-owning client plays it. Initial history is a baseline on start/resume;
repeated snapshots and archive moves do not replay alerts. Ordinary room sounds keep their
existing settings. In Settings → Sound notifications → YOU, the first row is **You**,
with the same checkbox and sound controls as the room rows. Enter or a checkbox click toggles
only developer-alert audio; Feishu and other alerts are unchanged. It defaults on, including
settings saved before the checkbox existed. Left/Right and the arrows choose and preview
**System default** or an installed system sound, even while the checkbox is off.
The name remains in `developer_sound`, and the checkbox is saved beside it as
`developer_sound_enabled`. `bus state` and `bus settings` expose
`developer_sound: {enabled, name}`, with a null name for System default.
System default prefers Glass when installed, then the first installed system sound.
A missing or unplayable selection falls back to that system default, never a bundled developer
sound. If no system sounds are installed, Bus logs that playback is unavailable; the other
notification routes remain active. No desktop popup is added.

Feishu bot DMs remain under Settings → NOTIFICATIONS and are always on.
A legacy `feishu_waiting_notifications: false` is read as true,
and `bus settings feishu-notifications --off` is rejected. The Settings row says **always on**;
it cannot toggle this route. Freeze diagnostics do not emit user notifications.
The recipient open_id defaults to `ou_ca1b530d792419a044e281f998ce68dc`.
Click **Recipient open_id** to edit it; **lark-cli path** overrides the executable,
with an empty field using `lark-cli` from PATH. Bus stores no credentials.
Configure bot authentication in lark-cli; sends explicitly use `--as bot`.

Only messages with the literal `[waiting-on-you]` or `[waiting_on_you]` tag reach Feishu;
untagged posts, `@you` mentions and the legacy phrase stay in the TUI. A fixed two-second
window combines tagged messages in a burst into one DM. Each contributes
`sender is waiting on you: action`, taking the first line or bullet after the marker (same or
following nonempty line), with Markdown closing markers skipped and the full action line
preserved. The same line from the same sender is not resent within 30 minutes.
Additional body lines and attachment paths are never added.
Routing markers and terminal controls are filtered. Each source contributes its own line.

The coordinator runs `lark-cli im +messages-send --as bot --user-id OPEN_ID --text TEXT
--idempotency-key KEY --format json` in one background worker without a shell, with a
ten-second timeout and cancellation. Failures log a warning and never block or crash Bus;
there are no automatic retries. Destination edits preserve pending messages, using the new
destination for the next send; an in-flight send completes with its captured destination.
There is no Feishu reply channel.

Tests use only scratch sessions, fake providers, a closed PATH recording bot CLI and
recorded sound effects. They never play audio, send real Feishu or automate the desktop.
