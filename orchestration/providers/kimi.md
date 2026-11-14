# Kimi Code: Bus-delivered room turns

These Kimi transport rules take precedence over conflicting dispatch or waiting instructions in the authored room prompt. Keep its workflow, authority, and reporting rules.

## Send and return to idle

Assign a worker with an ordinary foreground send:

```sh
bus send --room R --as ORCH --to WORKER --text "..."
```

Use the room and agent IDs from the current Bus message or `bus state`. Inspect the JSON result and retain the message and request IDs. A queued send is an assignment, not a completed result. Ask the worker to report with a fresh normal send to you in the named room, then end the turn so Bus can deliver that message as a fresh prompt. Do not use `--async`, `bus wait`, a background shell, or a polling loop to await worker results.

When a worker's normal result message starts your turn, inspect the original task with `bus message status MESSAGE_ID` and `bus history --room R`. Require `complete: true` and read the result before treating the task as done. An explicit `--reply-to` only stores an answer and settles the original request; it never queues a requester turn. A failure, cancellation, or abandoned request is not a successful result. If a send's result is uncertain, inspect its existing status before sending again.

## Reply and report

For a direct Bus message, reply in its room with its original message ID:

```sh
bus send --reply-to MESSAGE_ID --as YOUR_ID --text "..."
```

Run the send in the foreground. Claim settlement only when its JSON reports `ok: true` and `stage: "replied"`. If it fails, inspect `bus message status MESSAGE_ID` before retrying. The receipt reports `requester_turn_queued:false`. If the requester must act, deliver the result with a fresh normal `bus send --room ROOM --as YOUR_ID --to REQUESTER --text "..."` instead. The final turn text is not captured as a reply.

An answer to a developer message in MASTER belongs in MASTER. A worker's room result belongs in the work room. Send decisions and blockers that require developer attention to the developer in MASTER, following the authored reporting policy.

## Kimi session and permissions

Bus can launch Kimi before it has a session ID. The first delivered message creates the session. The Bus agent file supplies the initial system prompt; exact `--session` resume retains it. In Bus yolo mode, Kimi runs with `--auto` (Never Ask), including after resume; legacy `--yolo`, hidden `--yes` and `--auto-approve`, and a short-cluster `y` are replaced only in effective yolo argv, preserving other bundled options and values and replay unchanged on a normal run. Hidden `-C` and optional `-r` / `--resume` are parsed so they cannot hide that yolo bit. Bus refuses to save `--auto` together with any of those yolo aliases, so that combination cannot be created only to fail on a normal resume. Routine shell commands and edits proceed without approval; plan review can still wait when Plan mode is active. If a dialog blocks a task, report it rather than guessing an answer.
