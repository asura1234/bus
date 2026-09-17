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

The `send` receipt also includes the initial per-recipient request status. When
an earlier request still owns that agent, the new request reports
`reason: prior_request_active` and the exact `blocked_by_request_id`. The same
fields remain available from `message status`, so callers do not have to infer
the head-of-line blocker from a generic `queued` stage.

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
| `abandoned` | Bus settled the request without publishing a reply; inspect `reason` and `detail`. |

For Cursor, a completed turn whose `beforeSubmitPrompt` callback never produces
a matching trusted binding settles as `abandoned` with
`reason: cursor_submit_hook_unbound` after Cursor becomes idle. Bus does not
publish the unbound reply, but it releases the agent FIFO so later queued work
is not silently blocked forever.

Treat `complete: true` from `message status` or `wait` as the settlement signal.
A successful terminal write, a visually idle agent, or a queued focus change is
not proof that the request completed.

Both commands query the same persisted message status: `wait` owns no separate
completion state, and the correlated result remains queryable after the waiting CLI process exits or Bus restarts.
When Worker durably records that provider Request settlement, the same semantic fact wakes the room orchestrator.
The orchestrator receives the factual reply
and decides the next action; Bus does not interpret the reply or add a harness
polling loop, workflow notifier, or content-owned notification path. A future
`send --wait` convenience may only compose the existing durable `send` and
`wait` primitives.

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

Read the complete current terminal viewport for one managed agent:

```sh
bus agent read "$agent_id" --source visible
```

Or request exactly the amount of recent scrollback evidence you need:

```sh
bus agent read "$agent_id" --source recent --lines 80
```

The generic forms are `agent read AGENT --source visible` and
`agent read AGENT --source recent --lines N`. Visible returns the complete
current viewport and rejects `--lines`. Recent requires a positive caller-chosen
`N`; Bus does not choose or silently clamp a default. Both forms verify the
managed room, launch, terminal, session, and pane identity before and after the
native read, and fail closed without returning uncorrelated text if that identity
changes. Use terminal output as evidence for human or model judgment, never as a
substitute for message settlement or an automatic workflow signal.

When a managed coding agent is visibly waiting on a safe permission prompt,
observe the exact factual fingerprint before approving it once:

```sh
bus agent permission "$agent_id"
bus agent approve-once "$agent_id" \
  --fingerprint "$fingerprint" \
  --response allow-once
```

The generic forms are `agent permission AGENT` and
`agent approve-once AGENT --fingerprint FINGERPRINT --response allow-once`.
Approval is atomic, allowlisted, single-use, and identity-bound; stale, unknown,
or risky prompts send no keys. This surface does not expose arbitrary keystrokes
or grant reusable shell authority.

If current facts prove an idle agent still owns a historically wedged request,
the Human or an approved Orchestrator can invoke the same queue-preserving typed
recovery after checking its identity and revision facts:

```sh
bus request recover "$request_id" --confirm
```

The generic form is `request recover REQUEST_ID --confirm`. It abandons only the
exact confirmed current request and does not choose what happens to queued work.

Coding-agent harnesses can verify a trusted assignment outer frame without
using developer control:

```sh
bus assignment verify --frame "$frame"
```

The generic form is `assignment verify --frame FRAME`. A verified, absent, or
invalid result is factual assignment evidence; it does not select a skill,
fallback, or workflow action.

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
  `diagnostics`; `prior_request_active` includes `blocked_by_request_id`, while
  other reasons identify a busy, blocked, unavailable, or unready agent.
- **A request remains `awaiting_start`:** submission alone did not establish a
  trusted provider turn. Inspect diagnostics and recent agent output before
  deciding whether to retry.
- **`wait` times out:** retain its returned last status and continue with
  `message status`; a timeout does not prove failure or authorize duplicate
  work.
- **Agent terminal identity changed:** stop using `agent read` for that selector
  and inspect the owned session before taking action.

For the current command list and keyboard shortcuts, run `bus --help`.
