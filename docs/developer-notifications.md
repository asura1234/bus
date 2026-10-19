# Feishu waiting-on-you notifications

Bus sends a bot DM through `lark-cli` for a new developer-addressed MASTER message
containing `[waiting-on-you]` or `[waiting_on_you]`, or the whole phrase `waiting on you`
(including `still waiting on you` and `WAITING ON YOU`). Matching is case-insensitive.
Agent-addressed posts, work rooms, untagged progress and internal attention notices
without a waiting marker stay quiet. `[needs_developer_attention]` retains its Bus badge.

**Settings → Feishu notifications** is on by default, independently of **Freeze recovery
notifications**. The recipient open_id defaults to `ou_ca1b530d792419a044e281f998ce68dc`.
Click **Recipient open_id** to edit it. Click **lark-cli path** to override the executable;
an empty field uses `lark-cli` from PATH. Changes persist with other shared settings.
Bus stores no credentials. Configure bot authentication in lark-cli itself; sends explicitly
use `--as bot`, never the user identity. The equivalent toggle command is
`bus settings feishu-notifications --on` / `--off`.

A fixed two-second window combines a burst into one DM. Its plain text is
`orchestrator name is waiting on you: action`, taking the first line or bullet after
the tag or phrase (on the same or following nonempty line), preserving the full
action line. Markdown closing markers are skipped. No additional body or attachment
paths are added. Each tagged message in a burst contributes its own formatted line,
joined by newlines in one DM. Untagged messages do not join it. Recognized attention routing markers are
removed first; line endings and terminal controls are filtered.

The coordinator detects messages regardless of the visible room, terminal or focus.
A single background worker runs `lark-cli im +messages-send --as bot --user-id OPEN_ID
--text TEXT --idempotency-key KEY --format json` without a shell, with a ten-second
timeout and cancellation. Failures (missing CLI, authentication, network, invalid
response, timeout) log a warning and never block or crash Bus. There are no automatic
retries. Toggle or destination changes clear pending notifications; disabled and startup
history is not replayed. Archive moves and repeated snapshots do not replay a message.

Developer message notifications no longer emit OSC host-terminal popups. Freeze recovery
notifications keep their existing host-terminal behavior. Room sounds, unread counts and
composer drafts keep their existing behavior. There is no Feishu reply channel.
