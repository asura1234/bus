# Developer message notifications

Bus sends a desktop notification through the host terminal when a new message
addressed to the developer lands in MASTER and contains `[waiting-on-you]`
(case-insensitive), or the whole phrase `waiting on you` / `still waiting on you`
(case-insensitive, including `WAITING ON YOU`): an orchestrator's final answer to
a developer prompt, an agent's `bus send --room master --as AGENT --to human`
post, or a Bus-authored developer-attention notice with an identified origin.
Messages addressed to agents and messages in work rooms do not trigger it.

Settings → Developer message notifications is on by default, independently of
Freeze recovery notifications. Turning it off clears any pending burst. It is
saved with the other shared Bus settings. Unread counts and room sounds keep
their existing behavior; the composer is never changed.

Tagged messages notify with MASTER visible and focused, with another room or
an agent terminal open, and when terminal focus is unknown or unfocused.
Changing rooms or focus does not cancel a pending burst. Untagged messages do
not notify, including notices carrying only `[needs_developer_attention]`.
Disabled messages are not replayed later.
Startup history and archive moves do not create notifications.

A fixed two-second window starts with the first eligible message. A single
message shows `orchestrator name is waiting on you: action`, using the line or
bullet after the tag or phrase (on the same or next nonempty line), limited to 100
Unicode characters plus an ellipsis if truncated. Recognized attention routing
markers are removed first, matching MASTER chat. No additional body is sent.
Line endings and terminal controls are filtered. Multiple messages show
`orchestrator name is waiting on you: action and N more`, where N counts the
remaining tagged messages in the burst. The window is not extended by later arrivals. If output is busy, the
pending burst waits until it can be queued through Bus's ordered output owner.

The existing host notification encoder chooses OSC 9 (or Kitty OSC 99) and
wraps for tmux. This uses no osascript or separate notification process. The
host terminal's notification permissions and settings control presentation.
