# Developer notifications

Any worker or orchestrator may address the developer in any room with
`bus send --room ROOM --as AGENT --to you --text TEXT`. `human` remains an alias.
The agent-authored message stays in that conversation room, displays **You** as
recipient, and creates no agent request. Use @you or [waiting-on-you] for things only
the developer can do or decide.

Every direct agent message to you notifies, including untagged messages. Agent-authored
peer messages and completed agent replies also notify when they contain `[waiting-on-you]`,
`[waiting_on_you]` or the whole phrase `waiting on you` (case-insensitive, including
`still waiting on you`). Developer-authored sends never trigger alerts.

A separate bundled developer sound rings once for each qualifying source, regardless of
visible room, focus, room sound switches, startup grace or ordinary notification cooldown.
Only the coordinator-owning client plays it. Initial history is a baseline on start/resume;
repeated snapshots and archive moves do not replay alerts. Ordinary room sounds keep their
existing settings. No desktop popup is added.

Feishu bot DMs are always on. A legacy `feishu_waiting_notifications: false` is read as true,
and `bus settings feishu-notifications --off` is rejected. The Settings row says **always on**;
it cannot toggle this route. Freeze recovery notifications keep their separate toggle.
The recipient open_id defaults to `ou_ca1b530d792419a044e281f998ce68dc`.
Click **Recipient open_id** to edit it; **lark-cli path** overrides the executable,
with an empty field using `lark-cli` from PATH. Bus stores no credentials.
Configure bot authentication in lark-cli; sends explicitly use `--as bot`.

A fixed two-second window combines every qualifying message in a burst into one DM.
Tagged messages contribute `sender is waiting on you: action`, taking the first line
or bullet after the marker (same or following nonempty line), with Markdown closing
markers skipped and the full action line preserved. Untagged direct posts contribute
`sender to you: first line`. Additional body lines and attachment paths are never added.
Routing markers and terminal controls are filtered. Each source contributes its own line.

The coordinator runs `lark-cli im +messages-send --as bot --user-id OPEN_ID --text TEXT
--idempotency-key KEY --format json` in one background worker without a shell, with a
ten-second timeout and cancellation. Failures log a warning and never block or crash Bus;
there are no automatic retries. Destination edits preserve pending messages, using the new
destination for the next send; an in-flight send completes with its captured destination.
There is no Feishu reply channel.

Tests use only scratch sessions, fake providers, a closed PATH recording bot CLI and
recorded sound effects. They never play audio, send real Feishu or automate the desktop.
