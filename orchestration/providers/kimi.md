# Kimi Code: Bus-delivered room turns

These Kimi transport rules take precedence over conflicting dispatch or waiting instructions in the authored room prompt. Keep its workflow, authority, and reporting rules.

## Send and return to idle

Assign a worker with an ordinary foreground send:

```sh
bus send --room R --as ORCH --to WORKER --text "..."
```

Use the room and agent IDs from the current Bus message or `bus state`. Inspect the JSON result and retain the message and request IDs. A queued send is an assignment, not a completed result. Send other independent assignments, then end the turn so Bus can deliver the worker's reply as a fresh prompt. Do not use `--async`, `bus wait`, a background shell, or a polling loop to await worker results; a waiter may consume the result before its wake reaches you.

When a worker reply wakes you, inspect `bus message status MESSAGE_ID` and `bus history --room R`. Require `complete: true` and read the reply before treating the task as done. A failure, cancellation, or abandoned request is not a successful result. If a send's result is uncertain, inspect its existing status before sending again.

## Reply and report

For a direct Bus message, reply in its room with its original message ID:

```sh
bus send --reply-to MESSAGE_ID --as YOUR_ID --text "..."
```

Run the send in the foreground. Claim delivery only when its JSON reports `ok: true` and `stage: "replied"`. If it fails, inspect `bus message status MESSAGE_ID` before retrying. The final turn text is also captured by Bus; avoid sending the same answer twice.

An answer to a developer message in MASTER belongs in MASTER. A worker's room result belongs in the work room. Send decisions and blockers that require developer attention to the developer in MASTER, following the authored reporting policy.

## Kimi session and permissions

Bus can launch Kimi before it has a session ID. The first delivered message creates the session. The Bus agent file supplies the initial system prompt; exact `--session` resume retains it. In Bus yolo mode, Kimi runs with `--yolo`, including after resume. Routine shell commands and edits proceed without approval; Kimi may still ask for risky actions, questions, or plan review. If a dialog blocks a task, report it rather than guessing an answer.
