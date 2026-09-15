# How to use the Bus CLI

Bus provides two ways to work with rooms and coding agents:

- The interactive terminal UI, where people compose messages and inspect rooms.
- Developer control commands, which let a person, script, or another agent drive
  an already-running Bus instance and receive JSON results.

This guide covers both surfaces. It uses an installed `bus` command in examples.
When working from this repository, use `./run dev` instead of `bus --dev`, and
use `./run dev COMMAND` instead of `bus COMMAND`. The launcher rebuilds the
development binary, enables developer control, and forwards the command.

## Start or resume Bus

Start a new interactive session with developer control enabled:

```sh
bus --dev
```

Keep that terminal open. Run control commands from a second terminal. Control
commands connect to an existing `--dev` instance; they never start Bus or enable
control on a session that was launched without `--dev`.

A plain launch always creates a new local session. List saved sessions before
deciding which one to resume:

```sh
bus sessions
bus resume 0123456789abcdef --dev
bus resume --last --dev
```

`bus sessions` prints session IDs, room names, recent activity, and which session
was opened last. `resume` cannot be combined with `BUS_DATA_DIR`.

Use `bus --paths` to inspect Bus data, log, callback, configuration, and state
locations without starting a session.

## Target the intended running session

Without an override, a control command targets the last opened local session.
That is convenient for interactive use:

```sh
bus state
```

For scripts, tests, and concurrent Bus sessions, use a dedicated absolute
`BUS_DATA_DIR`. Launch the UI and every control command with the same value:

```sh
export BUS_DATA_DIR=/absolute/path/to/my-bus-session
bus --dev
```

Then, in another terminal:

```sh
export BUS_DATA_DIR=/absolute/path/to/my-bus-session
bus state
```

The override is an exact isolated root, not a request to discover or start a
session. Bus rejects relative paths.

## Create a room and add an agent

Every control command emits one JSON object. The examples below use `jq` to
capture IDs, because numeric IDs remain unambiguous even when names are reused.

```sh
room_id=$(bus room create "feature-work" | jq -r '.result.room_id')

agent_id=$(bus agent add \
  --room "$room_id" \
  --name "Implementer" \
  --provider codex \
  --pwd "$(pwd)" \
  | jq -r '.result.agent_id')
```

Supported providers are `claude`, `codex`, and `cursor`. `--pwd` selects the
agent's working directory. Use `--args STRING` for provider-specific launch
arguments.

Some projects require provider hook setup. Review the reported path and notice
before repeating `agent add` with `--consent-hooks`. If an existing agent is
waiting for the same approval, confirm it explicitly:

```sh
bus agent setup-confirm "$agent_id" --confirm
```

The `--confirm` flag is deliberately required for setup approval and deletion.

## Send work and wait for the reply

Send a task to one agent and retain the returned message ID:

```sh
message_id=$(bus send \
  --room "$room_id" \
  --to "$agent_id" \
  --text "Inspect the current change and report the most important risk." \
  | jq -r '.result.message_id')

bus wait --message "$message_id" --timeout 600
```

`send` confirms that the message was durably queued. It does not mean the agent
started or replied. `wait` polls every 200 milliseconds until every recipient
has replied or the timeout expires. Its timeout can be 1–600 seconds and
defaults to 60 seconds.

For non-blocking inspection, read the same state once:

```sh
bus message status "$message_id"
```

The per-agent `stage` explains how far delivery progressed:

| Stage | Meaning |
| --- | --- |
| `queued` | The request is persisted but has not begun submission. |
| `submitting` | Bus is writing the request to the provider terminal. |
| `awaiting_start` | Submission occurred, but no trusted provider turn start is bound yet. |
| `delivered` | A trusted provider turn started and Bus is awaiting its final reply. |
| `replied` | Bus recorded the final reply for that recipient. |

Treat `complete: true` from `message status` or `wait` as the settlement signal.
A successful terminal write, a visually idle agent, or a queued focus change is
not proof that the request completed.

## Address one or many agents

`--to` accepts a comma-separated list of agent names or numeric IDs:

```sh
bus send \
  --room "$room_id" \
  --to "2,3" \
  --text "Independently inspect the failure and return your evidence."
```

To address every agent currently in the room, say so explicitly:

```sh
bus send \
  --room "$room_id" \
  --to all \
  --text "Report your current result and any blocker."
```

Attach one or more files by repeating `--file`:

```sh
bus send \
  --room "$room_id" \
  --to "$agent_id" \
  --text "Use these artifacts as input." \
  --file /absolute/path/to/spec.md \
  --file /absolute/path/to/failure.log
```

Bus validates attachments before queuing the message.

## Inspect rooms, replies, and terminals

Read the durable message history for a room:

```sh
bus history --room "$room_id"
```

Read the recent terminal output for one managed agent:

```sh
bus agent read "$agent_id"
```

`agent read` returns up to 400 recent lines as text with ANSI styling removed.
It verifies the managed terminal identity first and fails closed if the terminal
changed. Use it for diagnosis or context, not as a substitute for message
settlement.

The control CLI always emits raw JSON. Agent reply text returned by `wait`,
`message status`, and `history` remains raw Markdown. The interactive room
history renders Markdown styling for agent replies only; human prompts remain
literal, and copying or quoting a reply preserves its raw Markdown source.

## Build reliable automation

Every command accepts a global `--request-id STRING`. Supply a stable unique ID
for a mutation when a caller may need to retry it:

```sh
bus room create "release-check" --request-id "release-check-room-v1"
```

Reusing the same request ID with the same operation is replay-safe while the
running process retains the receipt. Reusing it with different parameters
returns `id_conflict`. Generate a new ID for a genuinely new operation.

Reliable callers should also follow these rules:

1. Parse the JSON response and require `ok: true`; do not infer success from
   human-readable terminal output.
2. Capture room, agent, message, and request IDs and prefer IDs in later calls.
3. Use `--to all` only when broadcasting is intentional.
4. Use `wait` or poll `message status`; do not treat `send` as completion.
5. Preserve the last status returned with a timeout. It identifies the request
   and stage that still need investigation.
6. Sequence dependent tasks in the caller. Bus can fan a message out, but it
   does not infer dependencies between separate messages.
7. Keep publishing, destructive repository operations, and external side
   effects explicit in the task sent to an agent.

## Common workflows

### Give one agent a bounded task

Create or select a room, add an agent in the intended working directory, send a
specific task with its expected output, and wait on the returned message ID.
Use `history` for the durable conversation and `agent read` only when delivery
needs diagnosis.

### Fan out independent investigation

Add several agents to the same room, send the same evidence-gathering prompt to
their IDs in one command, and wait once on the shared message ID. The resulting
status contains one request entry per recipient, so a caller can distinguish a
completed agent from a blocked one.

### Run an implementation and code-review cycle

Use separate, sequential messages when later work depends on earlier work:

```sh
implementation_id=$(bus send \
  --room "$room_id" \
  --to "$implementer_id" \
  --text "Implement the approved change, run focused checks, and report changed files." \
  | jq -r '.result.message_id')
bus wait --message "$implementation_id" --timeout 600

review_id=$(bus send \
  --room "$room_id" \
  --to "$reviewer_id" \
  --text "Review the current working tree. Report only evidence-backed findings." \
  | jq -r '.result.message_id')
bus wait --message "$review_id" --timeout 600
```

If review finds a problem, send a new remediation message to the implementer,
wait for it to settle, and then ask the reviewer to verify the current state.
The same send/wait pattern also works for writing, research, debugging, release
checks, and other workflows; code review is not a special Bus mode.

### Resume human work

Use `bus sessions` to identify a saved session, then resume its UI with its
exact ID. After resuming with `--dev`, normal control commands target it as the
last opened session.

## Navigate or clean up

Queue a room or agent focus change in the interactive UI:

```sh
bus room focus "$room_id"
bus agent focus "$agent_id"
```

A successful focus response means the UI event was queued, not that a frame was
rendered.

Rename rooms or delete resources explicitly:

```sh
bus room rename "$room_id" "verification"
bus agent delete "$agent_id" --confirm
bus room delete "$room_id" --confirm
```

Deleting a room or agent is destructive. Resolve the target with `state`, prefer
its numeric ID, and pass `--confirm` only after checking it.

## Diagnose failures

Start with the durable control state and diagnostics:

```sh
bus state
bus diagnostics
bus message status "$message_id"
bus history --room "$room_id"
```

`diagnostics` reports the Bus version, developer-control state, storage and
coordinator errors, data and log locations, and each agent's status, wait
reason, actionable error, runtime identity, and current request.

Common failure patterns:

- **Control is unavailable:** confirm the intended Bus process is still running
  and was started with `--dev`, then verify every terminal uses the same
  `BUS_DATA_DIR` when an override is present.
- **A selector is ambiguous:** rerun `state` and use the numeric room or agent
  ID instead of its name.
- **Agent setup needs consent:** inspect the reported project hook path, then
  use `--consent-hooks` or `agent setup-confirm ... --confirm` only if intended.
- **A request remains queued:** inspect its `reason` and the agent entry in
  `diagnostics`; the agent may be busy, blocked, or have an actionable error.
- **A request remains `awaiting_start`:** submission alone did not establish a
  trusted provider turn. Inspect diagnostics and recent agent output before
  deciding whether to retry.
- **`wait` times out:** retain its returned last status and continue with
  `message status`; a timeout does not prove failure or authorize duplicate
  work.
- **Agent terminal identity changed:** stop using `agent read` for that selector
  and inspect the owned session before taking action.

For the current command list and keyboard shortcuts, run `bus --help`.
