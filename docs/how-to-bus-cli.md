# How to use the Bus CLI

Bus provides two ways to work with rooms and coding agents:

- The interactive terminal UI, where people compose messages and inspect rooms.
- Developer control commands, which let a person, script, or another agent drive
  an already-running Bus instance and receive JSON results.

If you are an agent orchestrating a room, also read the
[Orchestrator guide](orchestrator-guide.md).

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
was opened last. `resume` cannot be combined with `BUS_DATA_DIR`. A missing
`BUS_DATA_DIR` root is created owner-only (`0700`), as Bus's private control
socket requires.

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

## Use the MASTER room

Every session has exactly one MASTER room. It holds orchestrator agents: one
per work room at most, each assigned to the room it orchestrates. The human
chats with all orchestrators in MASTER. MASTER cannot be renamed or deleted, and
no work room can take its name. Old sessions gain MASTER when they are opened.

Any ROOM selector accepts `master`, in any case, for the MASTER room. Add an
orchestrator and assign it a work room in one step:

```sh
orchestrator_id=$(bus agent add \
  --room master \
  --name "claude-orch" \
  --provider claude \
  --pwd "$(pwd)" \
  --orchestrates "$room_id" \
  | jq -r '.result.agent_id')
```

Reassign or unassign it later:

```sh
bus agent orchestrate "$orchestrator_id" --room "$other_room_id"
bus agent orchestrate "$orchestrator_id" --none
```

The generic forms are `agent add ... --orchestrates ROOM` and
`agent orchestrate AGENT (--room ROOM | --none)`. Only agents in MASTER can
orchestrate. Assigning a second orchestrator to the same room fails until the
first is unassigned. Deleting a work room leaves its orchestrator in MASTER,
unassigned.

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

To record the message as written by a room agent instead of the human, add
`--as AGENT`. The author must be an agent in the same room, or the room's MASTER
orchestrator, and cannot also be a recipient; `--to all` skips it. The generic form is
`send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...] [--as AGENT]`.

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

Both commands query the same persisted message status: `wait` owns no separate
completion state, and the correlated result remains queryable after the waiting CLI process exits or Bus restarts.
The agent that sent the message reads the factual reply and decides the next
action; Bus does not interpret the reply or add a polling loop or workflow
notifier. A future
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

## Read session state

`state` returns the whole session in one JSON object:

```sh
bus state | jq '.result | {master_room, visible_room}'
bus state | jq '.result.rooms[] | {id, name, kind, unread_count, orchestrator}'
bus state | jq '.result.agents[] | {id, name, room_id, orchestrates, compactions}'
bus state | jq '.result.usage'
bus state | jq '.result.settings'
```

- `master_room` is the MASTER room's ID. `visible_room` is the room open in the
  UI, or `null` while a terminal or form is open instead. Deleting the visible
  room moves it to the room the UI falls back to, MASTER.
- Each room has `id`, `name`, `kind` (`master` or `work`), `notes`,
  `unread_count`, `sound`, `deletion_pending`, and `orchestrator`: the ID of
  the MASTER agent orchestrating it, or `null`.
- Each agent includes `room_id`, `status`, `details_disclosed`, `orchestrates`
  (the work room it orchestrates, or `null`), and `compactions`: `count` and
  `last_at_ms` of the provider context compactions Bus observed for that agent.
- `usage` has one entry per provider: `claude`, `codex`, and `cursor`. An
  `observed` entry reports `five_hour` and `weekly` windows with
  `used_percent`, `resets_at`, and `window_minutes`, plus `read_at_ms` and
  `observed_by_agent`. Codex usage is read after each Codex turn. Claude and
  Cursor usage is not collected yet. Status `unknown`, with a `reason`, means
  Bus has no data, never that the allowance is unused.
- `settings` holds the UI preferences, currently `color_blind_mode`.

## Sound notifications

Bus can ding when a new message lands in a room, like a group chat. A room rings
for an agent's final reply or a message an agent sent with `send --as`, never
for the human's own sends. Nothing rings for the first two seconds after start
or resume, and one ding covers a burst of messages.

MASTER starts with sound on; work rooms start off. Toggle a room from Settings
in the UI or with the CLI:

```sh
bus room sound "$room_id" --on
bus room sound master --off
```

The generic form is `room sound ROOM (--on | --off)`. `state` reports each
room's `sound`. Bus plays herdr's done sound once, from the client running the
session, and honors a custom `[ui.sound]` `path` or `done_path`.
`HERDR_DISABLE_SOUND` silences it.

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
changes.

An agent that is still `launching` can be read too, for example to see a
provider prompt such as Claude's "Do you trust this folder?" dialog that keeps
it from becoming ready. Its provider session has not started yet, so Bus checks
everything except the session and reports `runtime.session_verified: false`.
The CLI cannot answer such a prompt; use the terminal in the UI. Use terminal output as evidence for human or model judgment, never as a
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
the Human or an orchestrating agent can invoke the same queue-preserving typed
recovery after checking its identity and revision facts:

```sh
bus request recover "$request_id" --confirm
```

The generic form is `request recover REQUEST_ID --confirm`. It abandons only the
exact confirmed current request and does not choose what happens to queued work.

The control CLI always emits raw JSON. Prompt and reply text returned by
`wait`, `message status`, and `history` remains raw Markdown. The interactive
room history renders Markdown for every message, prompts and agent replies
alike; a newline typed in a prompt stays a line break. Copying a message or
quoting a reply preserves its raw Markdown source.

Attached files and images appear as absolute paths: in each `history` prompt's
`files`, and in the message-level `files` of `message status` and `wait`. Images
pasted into the Bus composer are saved under `attachments/` in the Bus data
directory, so those paths stay readable after the session ends.

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

Replace a room's notes, the free-text box under the room name in the UI. The
text replaces the whole field; pass `--text ""` to clear it. `state` returns each
room's current notes:

```sh
bus room notes "$room_id" --text "Goal: ship notes
Non-goals: UI changes"
```

Show or hide an agent's details in the sidebar, and switch the UI's color-blind
palette, the same toggles as in the UI:

```sh
bus agent details "$agent_id" --on
bus settings color-blind --off
```

The generic forms are `agent details AGENT (--on | --off)` and
`settings color-blind (--on | --off)`. `state` reports `details_disclosed` and
`settings.color_blind_mode`.

Clear a room's unread count without changing the room open in the UI:

```sh
bus room seen "$room_id"
```

Rename rooms and agents, or delete resources explicitly:

```sh
bus room rename "$room_id" "verification"
bus agent rename "$agent_id" "Code Reviewer"
bus agent delete "$agent_id" --confirm
bus room delete "$room_id" --confirm
```

Deleting a room or agent is destructive. Resolve the target with `state`, prefer
its numeric ID, and pass `--confirm` only after checking it.

Save and quit the interactive Bus, as Ctrl+Q does in the UI:

```sh
bus quit
```

A successful `quit` means the request was queued for the UI, which then saves
unsent drafts and shuts the session down. It does not wait for the exit.
A destructive command without `--confirm` fails and names the missing flag.

Deletion closes an agent's terminal only while the running server still
attributes it to that agent. If the server has already released it, for
example after the provider exited and its pane respawned a shell, Bus still
deletes the agent and its messages, leaves that terminal open, and lists it
under `terminals_left_open` in the result. Close it yourself if it is no longer
needed.

## Upgrading from the orchestrator build

Sessions saved by the build that had the built-in room orchestrator open
normally, with these changes:

- Each session gains the MASTER room. A work room already named MASTER, in any
  case, is renamed once to `Master (old)` and keeps all its data.
- A room's Room Brief goal and non-goals are appended once to its notes.
- The orchestrator's own transcript, workflow drafts, and capability grants are
  dropped, because those features no longer exist. Approve-once fingerprints it
  already sent stay single-use.
- `<root>/private/orchestrator-credentials.json` is no longer read. Delete it
  yourself if you no longer need the API key it holds.

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
