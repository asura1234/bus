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

Feishu is an optional local plugin. Install `feishu-bridge` on the same Mac and
configure its bot transport separately. Bus discovers it through **feishu-bridge path**
in Settings → NOTIFICATIONS (`feishu_bridge_path` in settings.json), or PATH when
the field is empty. Without the executable, both Feishu directions are off.
The recipient open_id defaults to `ou_ca1b530d792419a044e281f998ce68dc`.
Bus stores no credentials and does not invoke lark-cli directly.

Only literal `[waiting-on-you]` or `[waiting_on_you]` tags reach Feishu. Untagged
posts, `@you` mentions and the legacy phrase stay in the TUI. Requests get separate
DMs except when the same agent repeats the same normalized ask within thirty
minutes of a confirmed DM (with a returned message_id). Those repeats become durable aliases of its mapping;
the window survives restart and repeats do not extend it. Different recipients
remain separate. A failed or uncertain send cannot suppress a new explicit ask;
it gets a new DM and window. Command errors are persisted beside the attempted
marker and returned IDs. The message
contains `sender is waiting on you: action` and a short `bus:CODE` request code.
The complete action is preserved, with routing markers and terminal controls filtered.
Attachment files are not uploaded.

Bus persists the source prompt/request, conversation room, raising agent, exact
orchestrator binding, session root, recipient and stable idempotency key before
starting `feishu-bridge send --open-id OPEN_ID --text TEXT --idempotency-key KEY`.
The returned `message_id` and `chat_id` are committed to that record. Sends run
serially in a cancellable background worker. An attempted send is never replayed
automatically after a timeout, failure or Bus restart because its outcome can be
uncertain. Its request code remains available for explicit replies.

Configure the bridge's sink as:

```toml
sink_command = ["/absolute/path/to/bus", "feishu", "ingest", "--instance", "/absolute/bus/root", "--app-id", "cli_YOUR_APP"]
```

`--instance` names an exact private Bus session root or its local session registry
base. It is required: the sink never targets the last opened session or inherits
an agent's author identity. The bridge passes one v1 JSON object on stdin. The sink
validates the configured app ID, user sender type and p2p chat type, then Bus
checks the sender against its configured recipient and the recorded recipient/chat.
Reply ancestry (`reply_to`, then `root_id`, including accepted inbound ancestry)
or an explicit `bus:CODE` must identify exactly one request. Conflicting references
are refused. There is no matching by time, display name or most recent alert.

Accepted replies become developer-authored Bus messages with structured
`in_reply_to` provenance in history and the provider payload. They queue an
independent turn for the exact recorded orchestrator. Unknown/ambiguous replies
stay undelivered; deleted rooms/orchestrators are parked without selecting a
replacement. The deduplication receipt and queued message commit together.
Replays return the original receipt, including after restart; edited text does
not create a second instruction. Exit 0 from the sink means durable acceptance
(delivered or parked). An unavailable session or failed commit returns nonzero
so the bridge retains its spool and retries. Diagnostics stay in logs.

The bridge owns listener supervision, Feishu history reconciliation, offline
spooling and retry. Bus does not install/start the listener. Acceptance uses a
private scratch Bus, fake bridge and synthetic bridge input. The developer's
final live reply is a separate acceptance step; tests send no real Feishu messages.
