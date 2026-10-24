# How to use the Bus CLI

Bus provides two ways to work with rooms and coding agents:

- The interactive terminal UI, where people compose messages and inspect rooms.
- Control commands, which let a person, script, or another agent drive an
  already-running Bus instance and receive JSON results. Every session answers
  the commands in this guide; `bus --help` lists them.

If you are an agent orchestrating a room, also read the
[Orchestrator guide](orchestrator-guide.md).

This guide covers both surfaces. It uses an installed `bus` command in examples.

## Getting help

Use `bus <cmd> [<sub>] --help` (or `-h`) as the source of truth for a
command's flags, usage, and examples. For example:

```sh
bus agent --help
bus agent add --help
```

`bus --help` lists commands. A bare command group such as `bus agent` also
lists its subcommands. Help prints text, exits successfully, and does not need
a running session.

## Start or resume Bus

Start a new interactive session:

```sh
bus
```

Keep that terminal open. Run control commands from a second terminal or from an
agent inside Bus. Control commands connect to an existing session; they never
start Bus.

A plain launch always creates a new local session. List saved sessions before
deciding which one to resume:

```sh
bus sessions
bus resume 0123456789abcdef
bus resume --last
```

`bus sessions` prints session IDs, room names, recent activity, and which session
was opened last. `resume` cannot be combined with `BUS_DATA_DIR`. A missing
`BUS_DATA_DIR` root is created owner-only (`0700`), as Bus's private control
socket requires.

Use `bus --paths` to inspect Bus data, log, callback, configuration, and state
locations without starting a session.

On resume, Bus relaunches each agent into the conversation its own hooks last
bound, even when the terminal saved an older one. Codex and Cursor read their
Bus hooks from the project's `.codex/hooks.json` and `.cursor/hooks.json`, and
any Bus that adds an agent in the same project moves those hooks to its own
executable. Resume moves them back to the resuming Bus. It never recreates hooks
that were removed: that agent stays suspended until you add it again.

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
bus
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
capture IDs, because IDs stay stable when an agent is renamed or replaced.

```sh
room_id=$(bus room create "feature-work" | jq -r '.result.room_id')

agent_id=$(bus agent add \
  --room "$room_id" \
  --name "Implementer" \
  --provider codex \
  --pwd "$(pwd)" \
  | jq -r '.result.agent_id')
```

Current agent names are unique across every room in a session, including MASTER.
Names are case-sensitive and surrounding whitespace is trimmed. Creation and rename
reject a name already used by another current agent, reporting its ID and room.
A name becomes available after its agent is fully deleted; historical identities remain.

When resuming an older session with duplicates, the lowest-ID agent keeps its name.
Later duplicates receive `NAME-ID` (or a numbered suffix if that name is taken).
Existing distinct names, IDs, provider sessions, room bindings and queued work stay
intact. Each rename is recorded once in its home room history before clients connect.

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

Every session has exactly one MASTER room. It holds orchestrator agents: each
is bound to exactly one work room for its whole life, and a work room has at
most one orchestrator. The developer
chats with all orchestrators in MASTER. MASTER cannot be renamed or deleted, and
no work room can take its name. Old sessions gain MASTER when they are opened.

Any ROOM selector accepts `master`, in any case, for the MASTER room. Create the
work room first, then add its orchestrator with `--orchestrates`, which is
required in MASTER:

```sh
orchestrator_id=$(bus agent add \
  --room master \
  --name "claude-orch" \
  --provider claude \
  --pwd "$(pwd)" \
  --orchestrates "$room_id" \
  | jq -r '.result.agent_id')
```

Orchestrators run on Claude Code or Cursor. Codex cannot be an orchestrator
yet: an orchestrator dispatches work with `bus send --async` in the background
and must be woken when it finishes, but Codex does not start a new turn when a
background command exits (openai/codex issues 32188, 50079). The MASTER Add
agent form offers only Claude Code and Cursor, and `agent add --room master
--provider codex` fails with that explanation. Codex works fine as a worker in
a work room, and Codex orchestrators already in a saved session keep loading
and resuming.

The PWD works as for any agent; Bus writes nothing into it. Every MASTER agent
launches with an orchestrator system prompt, by default Bus's built-in one
(`orchestration/prompt.md` in the Bus repository). Replace it with
`--system-prompt TEXT` or `--system-prompt-file PATH`. Bus fills in
`{{ROOM_NAME}}`, `{{ROOM_ID}}`, `{{AGENT_NAME}}`, `{{DOCS}}` (the session's
`<BUS_DATA_DIR>/docs/` folder), and `{{WORKFLOWS_DIR}}` (the resolved workflow
directory). The docs include this guide, the orchestrator guide,
`workflow-create.md`, templates and every shipped workflow. See the
[binding named-workflows rule](orchestrator-rules.md#named-workflows) for the
directory and shipped names.

Bus delivers the prompt with each provider's own launch option and passes it
again on resume:

| Provider | Delivery |
| --- | --- |
| Claude Code | `--append-system-prompt-file`, added to Claude Code's default prompt |
| Codex (saved sessions only) | `-c developer_instructions=...`, a developer message beside Codex's base instructions that points Codex at the prompt file, so the launch command Bus types stays short |
| Cursor | No launch option exists, so Bus sends the prompt as the agent's first message |

The prompt is kept in the launch's callback folder as `system-prompt.md`. A new
PWD can show the provider's "trust this folder" prompt on first launch; answer
it in the agent's terminal. Bus does not pre-trust folders.

There is no reassignment: the system prompt names the room at launch, so an
orchestrator keeps it for life. Deleting the work room also deletes its
orchestrator (its terminal is closed and it leaves MASTER); to orchestrate
another room, add a new orchestrator.

Additional launch args (`--args`, or the form's Args field) work for MASTER
agents exactly as in any room and combine with the Bus-owned prompt arguments.

### Move an existing session into MASTER

An agent can adopt an existing provider session instead of starting a new one,
for example to turn a Claude Code conversation into an orchestrator. Pass the
session's ID (a UUID) in the launch args, in the provider's resume form:

| Provider | Launch args | Where to find the ID |
| --- | --- | --- |
| Claude Code | `--resume SESSION_ID` | `/status` in the session |
| Codex | `resume SESSION_ID` (first); workers only | `/status` in the session |
| Cursor | `--resume SESSION_ID` | `cursor-agent ls` |

Codex cannot be an orchestrator (see above), so only a Codex worker in a work
room can adopt a session.

```sh
bus agent add --room master --name claude-orch --provider claude \
  --pwd /path/the/session/ran/in --orchestrates "$room_id" \
  --args "--resume 160d1f8b-9023-44b8-9bc7-24333effb185"
```

- Quit the session wherever it runs first; two processes must not share it.
- Use the session's original PWD: Claude Code finds sessions by directory.
- Bus rejects anything but one UUID: no picker, `--continue`, `--last` or
  `--fork-session`, and a session already bound to another Bus agent.
- The provider's session-start hook binds the session to the new agent, so
  callbacks, delivery and resume after a restart work as for a fresh agent.
  Cursor runs no session-start hook on `--resume`, so Bus binds an adopted
  Cursor session from its launch args; later callbacks must still match it.
- The orchestrator prompt still applies. Claude Code gets
  `--system-prompt-snapshot off` with the prompt file, because a resumed
  conversation otherwise replays the system prompt it started with. Cursor
  takes the prompt as its first message.

The generic form is `agent add --room master ... --orchestrates ROOM
[--system-prompt TEXT | --system-prompt-file PATH]`. Only agents in MASTER can
orchestrate, and every MASTER agent must. Adding a second orchestrator for a
room that already has one fails.

### Report to the developer in MASTER

A reply lands in the message's room, equally for developer and agent senders.
An agent-authored message sent in MASTER gets its reply in MASTER, just like
one from the developer. A worker message in the work room gets its reply there.
A turn no message started posts in the orchestrator's work room.

Direct Bus control commands (`send`, `wait`, `history`, `agent`, `room`,
`message`, `request`, `state`, `settings`, and other control commands) do
not hold Claude's turn-final reply while running in a background shell. A
quoted Bus executable path and a leading `cd` or environment assignment are
supported. Real work before or after a Bus command, pipes to other programs,
setup loops, unknown syntax and opaque script wrappers still hold the reply.
For dispatches use the direct Bus executable; do setup and result processing
separately. With dev logging or `BUS_LOG=bus=debug`,
`bus.callback.shell_classified` records each running shell's id,
content-free reason and hold decision. At info level,
`bus.callback.background_shells` adds
the request, sender kind and captured-reply size to explain a held final;
these events never log commands or message text.

### Who to message

Who you message, when, and how to write it is in the
[Bus communication guide](bus-communication-guide.md). This page only covers
the commands.

To post a milestone in MASTER as an orchestrator:

```sh
bus send --room master --as claude-orch --to human \
  --text "Merged PR #N. [waiting-on-you] Please restart Bus and run the smoke test."
```

The message shows in MASTER history as written by that agent, addressed to
you; it counts as unread and triggers the mandatory developer alert. It goes to no agent, so it
has no requests and nothing to `wait` on: `send` returns its `message_id`
with `"request_ids": []` and `"stage": "posted"`. `--file PATH` attaches files
as for any send. `human` is a compatibility alias for `you`; both work from every room.

#### Reach the developer from any room

For a developer-only request from any room:

```sh
bus send --room ROOM --as AGENT --to you --text "[waiting-on-you] Approve access."
```

`@you` means the developer; use `--to you` in the CLI. The message stays in the selected
conversation room, with the usual agent attribution and a **You** recipient.
No copy is moved to MASTER. Agent names or IDs work for `--as`, including senders
from another room. `you` and `human` are case-insensitive reserved names.

These posts create no agent requests: the result has `request_ids: []` and `stage: posted`.
`--as AGENT` is required; mixed developer/agent recipients, `--queue` and `--async`
are rejected. The developer cannot send to themselves.

Every agent post to you, plus every agent message or final with an explicit `@you`,
`[waiting-on-you]` or `[waiting_on_you]` marker, shows as unread for the developer in the TUI
and plays the developer-alert sound once per batch. Only the `[waiting-on-you]` or
`[waiting_on_you]` tag also sends a Feishu DM; use it only for hard blockers. For the TUI alert only, the legacy whole phrase `waiting on you` also counts in developer-addressed
notices and replies to developer-authored requests. In peer traffic a bare phrase does not trigger developer alerts
because "you" refers to the receiving agent. Developer-authored sends never notify.
The developer-alert system sound is configured separately from ordinary room sounds and ignores
room switches and normal notification cooldown when enabled. In Settings → Sound notifications →
YOU, **You** has the same checkbox and sound controls as room rows. Click the checkbox or use
Enter to toggle developer-alert audio; Left/Right or the arrows choose and preview System default
or installed system sounds. The checkbox defaults on for existing setups and is saved as
`developer_sound_enabled` beside the existing `developer_sound` name. Unchecking silences only
this audio; Feishu and other alerts continue. `bus state` and `bus settings` expose
`developer_sound: {enabled, name}`, with null for the System default name.
System default prefers Glass, then another installed sound; missing selections fall back to it.
Feishu bot DMs carry only tagged asks, in the two-second burst and first-action-line format;
the same ask from the same sender is not resent within 30 minutes. Feishu notifications remain mandatory, including with a saved
legacy Feishu off preference.
Recipient and CLI path remain editable. See [developer notifications](../docs/developer-notifications.md).

## Repository status reporting belongs to the agent and harness

Bus does not discover repositories, poll a code host, interpret PR/merge-request
state or decide the next workflow action. Use your agent and harness tools for
GitHub, GitLab, Aliyun or another repository host.

For assigned repository work, report these meaningful transitions yourself:

- your PR/merge request is merged;
- it becomes ready for review;
- CI finishes, with its result;
- a new merge conflict appears;
- an upstream merge materially affects your assigned work.

Include repository, PR/merge-request number, exact head when available, status
and the next action or owner. Send one update per meaningful transition; do not
broadcast every merge in an unrelated repository. Route reports according to the
[Bus communication guide](bus-communication-guide.md): workers report to their orchestrator, not the
developer. A missing orchestrator or send failure is not permission to send routine
status to the developer. These are agent-authored reports, not Bus-generated notices.

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

`wait` defaults to 60 seconds and accepts `--timeout` from 1 through 600.
Use `bus wait --message "$message_id" --async` to wait without an overall
deadline until the message is complete, a request is abandoned or stalls, or a
recipient needs a dialog. Like `send --async`, it succeeds only on
`complete: true`; a turn Bus closed as `reply_not_captured` counts as done. It returns
`agent_waiting_on_dialog` with the last status; answer the dialog, then wait
again. `--async` and an explicit `--timeout` cannot be combined. Unlike
`send --async`, `wait --async` stops at a dialog rather than waiting for it to
be answered. Each transport request still has its normal finite timeout;
`wait --async` retries transport failures after a one-second pause, like
`send --async`. Bounded waits still report transport failures immediately.
To obtain a message ID before waiting, use plain `send`, then `wait --async`.

To record the message as written by an agent instead of the developer, add
`--as AGENT`. Any agent can message any other agent, across work rooms and
MASTER. The author cannot also be a recipient; `--to all` skips it. The generic
form is `send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...]
[--as AGENT] [--queue] [--async]`. The message, history and reply stay in the
conversation room chosen by `--room`.

Names and numeric agent IDs resolve globally across every room, including
MASTER. An exact existing numeric ID wins over a numeric name. `--room` chooses
the conversation; it never narrows the current agent-name namespace. `--as`
identifies the author using the same shared name/ID resolver.

A worker can message its orchestrator before being assigned any work:

```sh
bus send --room "$room_id" --as "$agent_id" --to orchestrator \
  --text "Ready for verify; please check the implementation."
```

The case-insensitive reserved `orchestrator` alias resolves to the orchestrator
bound to the sender's own home room, even when `--room` chooses another
conversation room. A real member of the conversation room, then the home room,
named `orchestrator` takes precedence. With no matching member and no binding,
Bus reports `This room has no bound orchestrator`. Other orchestrators, MASTER peers and workers in any
room are reachable by name or ID.

Bus prepends exactly one sender line to agent-authored payloads: `name:` for
same-room delivery, and `name (room R):` when the conversation or recipient
crosses rooms or involves MASTER. Numeric names and historical names retained from before a rename include
`(agent ID)` so the sender can still be addressed unambiguously. Batched agent messages use `[N/M at HH:MM]` without repeating the
sender; developer/Bus messages without an inner header retain their sender in
the batch marker.

Send only the message body: do not add a "from X", "X here" or signature line.
An exact current or legacy Bus header with this sender's own name, ID and home
room, or a short identity-only line (`name:` or `agent ID here —`), is normalized
to the genuine current header. Ordinary sentences and headers with a forged ID
or room keep their content and receive the real header. Stored chat text stays
as authored; delivered payloads and callback matching use the same frozen text.

Every CLI agent selector accepts a globally unique current name or numeric ID,
including `--as`, `agent dialog/choose/answer/read/delete/interrupt/rename/setup-confirm/clear`,
and the dev commands `focus/details`. The selector commands also accept optional
`--room ROOM` and `--as AGENT` context; names remain global. Exact target IDs do
not require resolving that optional author context. For interrupt, `--room`
chooses the conversation for the receipt and follow-up text, while the interrupt
notice stays in the target agent's room where its turn ran.
`agent interrupt --room ROOM` without AGENT stops that room's eligible agents.

```sh
bus agent list
bus agent list --room "$room_id"
bus agent dialog reviewer --room "$room_id" --as owner
```

`agent list` emits JSON mapping rows with `name`, `id` and `room`. Message
and request selectors remain numeric resource IDs; an agent-valued
`--as` on message/wait commands uses the same global name/ID resolver.

New agents receive Bus protocol instructions at launch. Existing conversations
keep their saved instructions; upgrades do not insert new task turns.

A message from another conversation room waits for the recipient's running
turn to finish, then gets its own turn and reply. Same-room steering,
`--queue`, `wait` and settlement keep their normal behavior.
Bus's blocked-dialog notices take priority and may steer into an orchestrator's
active turn across rooms. If a notice remains unsubmitted or its native write
unconfirmed for two minutes, Bus reports it to the developer even while the
orchestrator is working or blocked. A readable closed dialog suppresses fallback.
If its dialog state cannot be read, Bus allows at most three extra minutes,
then tells the developer the state is unreadable and the agent may still be
blocked. Uncertain writes are never replayed.
`--to all` targets conversation-room members only (excluding `--as`). The
work-room composer's picker lists the bound orchestrator separately; its All
row selects and clears room members only.

`send` confirms that the message was durably queued. It does not mean the agent
started or replied. `wait` polls every 200 milliseconds until every recipient
has replied, a recipient waits on a dialog, a recipient's message stalls (exit
3), or the timeout expires. Its timeout can be 1–600 seconds and
defaults to 60 seconds.

### Send and follow a task with `--async`

`send --async` sends as usual, then blocks with no time limit until the message
is complete: every recipient replied or Bus closed its request. It emits one JSON
object containing the final `message status`, `message_id`, and the original
send receipt in `result.send_receipt`:

```sh
bus send --room "$room_id" --as "$me" --to "$agent_id" --async \
  --text "Implement the parser and report what you changed."
```

It succeeds only once `message status` reports `complete: true`. A
recipient's turn has ended (`turn_ended: true` in `message status`) once,
after the message was submitted, Bus saw it Working or Blocked or its provider
reported the turn start for the message, and Bus has seen it Idle since. The
provider's turn start covers a turn that starts and ends between two status
polls. A captured reply also counts. An ended turn is not yet done: a message
typed into a running turn reads ended for a few seconds while Bus holds the
group's reply, and the command waits until it is `replied`. A turn that ended without a captured
reply is `settling: true` for a few seconds while Bus waits for a late reply;
the command waits for that settlement too. If the provider ended the turn on an
error (a Codex model at capacity, say), Bus then abandons the request with reason
`provider_error: <message>` and the command exits 1 with code
`message_provider_error`, which is worth a retry. A Claude API error (such as
`API Error: Connection lost mid-response` or `529 Overloaded`) closes the
request at once with the same reason, carrying Claude's error text and
category. Without a provider error,
settlement abandons it with reason `reply_not_captured`, which `--async` counts
as done and succeeds on. If the agent went unavailable after its turn ended (its
provider exited, say), settlement abandons it with reason `agent_unavailable` and
the command exits 1 with `message_abandoned`.
A blocked recipient, for example one waiting on a dialog, is not idle, so the
command keeps waiting until someone answers it. Both async waits retry live-server
overload, including accepted connections closed without a response. Four consecutive
transport failures exit 1 with `bus_down`; a live status reply or accepted-connection
disconnect resets this budget. It exits early only if Bus is unreachable, the send
fails or Bus abandons a recipient's request other than as `reply_not_captured`
(exit 1), or a recipient's message
stalls before it settled (exit 3, see Stalled messages); the reason is in
the JSON error object.
`--async` cannot be combined with `--to human`.

Orchestrators run it as a background tool call (Claude Code: Bash with
`run_in_background`; Cursor: a background shell with timeout 0), so their CLI
wakes them when it exits and prints the captured replies. Bus also maintains
work-room situation awareness through the same agent-message queue as Blocked
notices. It delivers these worker events to the room orchestrator's terminal:

- A captured reply to an orchestrator-authored message, including a late final
  after that request closed without a reply.
- A turn-final message from an autonomous turn nobody's request started, such as a
  background job's wake turn. Its full text becomes a worker message in work-room
  history. A worker's reply to another worker's request goes to that worker only.
- A request to a work-room worker that stalls or ends without a reply, with its
  reason, including `reply_not_captured` and provider errors. Requests ended by
  `bus agent interrupt` or by deleting the agent report nothing.
- Confirmed process exit with an open orchestrator request.

Bus sends no idle-worker or idle-request nudges: an agent waiting is not a problem.

Each notification names the worker, event and message/request reference. Bus
collects same-worker events for two seconds and combines them in one short
message; new events can join a still-queued batch during its collection window.
Long text uses a preview and `bus message status MESSAGE_ID` or
`bus history --room ROOM` pointer. Different workers keep their own identity.
No room monitoring, self-timer or surviving waiter is required. Blocked,
Resolved notices keep their existing behavior. Startup failures are retained in agent state and diagnostic logs.

An author's `send --async --as ORCH` renews a three-second waiter lease, so its
reply and no-reply outcome components wait for the combined output.
`wait --message ID --as ORCH` opts into the same behavior. A plain `wait` or
`message status` cannot suppress delivery. After stdout flush succeeds, the
waiter marks only printed completed replies and the exact printed outcome
`event_acks` keys seen. Status `events` includes all retained group summaries,
including finals, exits and already-seen events; only unseen own outcomes are
acknowledgeable through `event_acks`.
It cannot hide an event that appeared after its output, an autonomous result,
idle, an exit, or another message's events. Seeing a shared reply covers its group.
A killed waiter stops renewing its lease, so unseen components are routed.

Event decisions and queued batches are saved together before native submission.
Pending events, callbacks spooled while Bus was down and queued batches recover
on restart without allocating another notification for the same event. Pending
sources and full-text pointer targets do not expire before delivery. Repeated
callbacks for the same provider turn, repeated idle/exit polls and restarts do
not create duplicate events. A new outcome or a later turn is a distinct event.
Definite native rejection retains the same queue id; after the queued stall
window retries pause until a new message, status/identity change or restart.
Idle polls alone do not resume pasting; uncertain submissions are not replayed.

A post-flush acknowledgement removes its component from a still-queued mixed
batch and preserves the others; an empty batch becomes abandoned in history.
Once submission starts its payload is immutable. stdout and terminal injection
have no shared transaction: a late waiter still prints a reply already sent as
a wake, while ordinary live waiters avoid that overlap through lease and Seen.
Bus submits each event notification once under the existing uncertain-outcome
policy and favors recovery when an output acknowledgement fails.

The first upgrade backfills retained eligible replies and no-reply outcomes
once because older versions saved no seen record. This can include older
retained outcomes, coalesced per worker. Seen/delivered sources follow ordinary
retention; expired physical history is not resurrected. Rolling back to a
binary that discards routing state can repeat recovery. Replies to
developer-authored messages and MASTER reporting keep their existing behavior.

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
| `joined` | The message joined another message's turn; see `group`. |
| `replied` | Bus recorded the final reply for that recipient. |
| `abandoned` | The request is closed and will not run later; `reason` explains a queued delivery failure. |
| `stalled` | The message made no progress for a grace period; `reason` says why and `stalled_from` names the stage it stalled in. |

### Provider exits

When an agent's provider exits without Bus asking (not on delete, and not
during startup, which reports its own screen), Bus first saves the pane's
visible screen and its last 200 lines of scrollback to
`<data dir>/exit-snapshots/a<agent id>-<unix ms>.txt`, one file per exit. Once
that record is saved, it closes the terminal it created for a work-room agent,
even if the shell left behind is running something (an updater prompt, a
command); a MASTER orchestrator's terminal stays open. The exit notices carry
the snapshot path and the last 20 non-blank lines (at most 160 characters each
and 1,600 in all; the file has the whole screen):

```text
Exited with open request (process_exited); message 41 (request 42).
Inspect: bus message status 41
Exit snapshot: <data dir>/exit-snapshots/a7-1792482896455.txt (Bus closed its terminal).
Last lines:
> codex: fatal: stream disconnected
> [oh-my-zsh] Would you like to update? [Y/n]
```

The MASTER room gets `Agent process exited: <name> (agent <id>).` with the same
snapshot lines, also when the worker had no open request. A message sent to the
agent after its exit gets the same exit notice. Quoted screen lines never page
anyone: their `waiting on you` markers are written with no-break spaces. If the
pane could not be read, the notice says `Exit snapshot unavailable: <reason>`
and the terminal stays open for you to inspect. `bus state` shows the agent's
`exit_snapshot` (`path`, `tail`, `at_ms`, `launch_id`, `terminal_id`, or
`error`; `terminal_closed` is `true` once Bus closed the terminal, `false` when
it left it open, `null` before it tried), and each request in
`bus message status` carries its agent's `exit_snapshot`. `bus agent read`
cannot read an exited agent; read the snapshot file instead.

### Recover a room after a provider outage

Bus disables startup updates for Codex, Claude Code and Cursor on fresh launches and
resumes, including restored panes outside rooms. Explicit Claude updater environment
settings retain precedence. Update shared provider installations separately while
their agents are stopped.

When a launch reports a missing provider command, Bus records one `BLOCKED` notice
per room/provider outage, naming the binary. Affected agents and
their tasks remain saved. Once the provider works again, run:

```sh
bus agent resume --room ROOM
```

This relaunches unavailable agents with their saved provider sessions, saved options
and capture hooks. It skips providers still running or launching, preserves queued
work and never replays an already typed task. Inspect each returned agent's
`outcome` (`resuming`, `skipped` or `blocked`); an unconfirmed close, missing session
or invalidated ownership needs inspection before another launch. A retry does not
interrupt a provider which already recovered.

### Stalled messages

A message stalls when it stops moving while its agent is not working or
blocked. Bus closes an untyped queued request after its grace period: its
stage becomes `abandoned`, `complete` becomes `true`, and `reason` records the
failure. It cannot fire later, even after a restart. Fix the cause and send a
new message when ready. Typed requests remain `stalled` for explicit recovery.

| `stalled_from` | After | `reason` |
| --- | --- | --- |
| `queued` | 2 minutes with the agent not working | The native server's last refusal to type it (`input_box_not_empty`: the input box holds someone's text; `agent_not_ready`; `agent_blocked`), otherwise why Bus is waiting (`session_hook_missing`, `agent_unavailable`, `prior_request_active`, …) |
| `submitting` | 3 minutes idle after Bus typed it | `submission_unconfirmed`: the native server never confirmed the typing |
| `awaiting_start` | 3 minutes idle | `no_start_hook`: the provider never reported the turn start |
| `delivered`, `joined` | 3 minutes idle with no hook event | `transcript_unmatched`: the turn ended but no final reply matched the transcript |
| Any typed stage | 3 minutes without progress after the agent becomes unavailable with its session binding invalidated | `session_invalidated`: the turn cannot continue in that session; recover the current lead request with `request recover REQUEST_ID --confirm` to close its group |

A captured reply held for background shells its own turn started does not
stall while that current shell hold lasts. The `background_shell_hold_minutes`
cap settles the hold with its latest interim reply if the shells never finish;
non-queued follow-ups can join the held turn. An uncapped cron hold is still
subject to stalled-status reporting.

A Working agent never stalls, however long its turn. A Blocked agent does not
stall either: it already asked for help. After 5 minutes blocked, `reason`
reads `blocked_unanswered` so the block shows in `message status`, but the stage
stays as it was and `send --async` keeps waiting. For a typed stalled request,
fix the cause (clear the input box, answer the agent, restart it), or recover
the request explicitly with `request recover`. `wait` and `send --async` stop
at a typed stall with exit code 3 and the reason in the JSON error; an abandoned
queued request exits with code 1. For `--async`,
a finished turn whose reply was not captured counts as done once settlement
abandons it as `reply_not_captured`. A bounded `wait` needs the
reply: it fails on any abandoned request, with `message_provider_error` for a
provider error and `message_abandoned` otherwise (exit 1).

Treat `complete: true` from `message status` or bounded `wait` as the durable
settlement signal; async waits succeed only on it.
A successful terminal write or a visually idle agent is
not proof that the request completed.

### Steer an agent while it works

You never have to wait for an agent to finish before correcting it. A message
sent to an agent that is working on a Bus message is typed into its terminal
right away, the way a person types while an agent works, and the provider takes
it into the running turn:

```sh
bus send --room "$room_id" --to "$agent_id" --text "Use the streaming API instead."
```

The message joins the agent's current group: the message that started the turn
plus every message typed into it. The turn's final reply belongs to the whole
group. `message status` and `wait` for any message in the group return that
shared reply and `complete: true` once the group settles; each joined request
reports the first message's request ID as `group`. The room history shows the
group's messages stacked, with one reply under the newest.

Verified live with each provider:

| Provider | What happens to the typed message |
| --- | --- |
| Claude Code | Taken into the running turn at its next step; the submit hook reports it under the turn's prompt ID. |
| Codex | Taken into the running turn; the submit hook reports it under the turn's ID. |
| Cursor | Runs as the next generation once the current one ends; that generation's reply answers the group. |

Bus does not type into an agent that waits on a dialog or a blocked screen;
the message stays queued, and the agent's "Blocked, needs help to continue."
message tells you to look at its terminal.
A message sent to an idle agent starts a new turn and group, as before. When
messages piled up while the agent could not take them (it was launching,
blocked, or finishing another turn), Bus sends them as one prompt, in order,
each part headed `[N/M at HH:MM]` for agent-authored messages, whose inner
payload carries the sender header. Developer/Bus parts retain
`[N/M from SENDER at HH:MM]`. The parts form one group.

Add `--queue` to wait for the agent to become idle and give the message a turn
of its own, with its own reply. A `--queue` message is never typed into a
running turn or joined with other messages. In the room UI, Enter steers and
Option+Enter (Alt+Enter) queues.

Both commands query the same persisted message status: `wait` owns no separate
completion state, and the correlated result remains queryable after the waiting CLI process exits or Bus restarts.
The agent that sent the message reads the factual reply and decides the next
action; Bus does not interpret the reply or add a polling loop or workflow
notifier. A future
`send --wait` convenience may only compose the existing durable `send` and
`wait` primitives.

### Interrupt an agent

A message never stops a turn: Codex takes it into the running turn and Claude
Code queues it for its next step. To stop a worker, or to redirect it onto
something else, interrupt its turn:

```sh
bus agent interrupt "$agent_id" --as "$me" --text "Stop. Reply with your status."
```

Bus presses the provider's stop key in the agent's terminal, once: Esc for
Claude Code and Codex, Ctrl+C for Cursor, the keys each shows as its own
"esc to interrupt" / "ctrl+c to stop" hint. A second press would open Claude's
rewind, Codex's edit-previous-message or quit Cursor, so Bus never repeats it.
It then waits up to 15 seconds for Bus to see the agent idle and reports one
JSON object with `outcome`:

| `outcome` | Meaning |
| --- | --- |
| `stopped` | The turn ended after the key. |
| `already_idle` | The agent was not working; nothing was typed. |
| `failed` | `reason` says why: the agent is launching or unavailable, it was still working after 15 seconds, or the key could not be sent. The command exits 1 with code `interrupt_failed`. |

The interrupted message, the messages typed into its turn and those queued for
the agent close with reason `interrupted`; nothing replays later. A `wait` or
`send --async` on one of them ends with error code `message_interrupted`
(exit 1) rather than waiting on, and `message status` reports stage `abandoned`
with reason `interrupted`. If the agent finished and Bus captured its reply
before the key took effect, that reply still completes the message. An idle
agent's request that Bus still holds (a settling turn, or one waiting on
background shells) closes the same way. The room history records the
interrupt as a message by the interrupter (`--as`, otherwise you) reading
`Interrupted NAME.`

`--text` is sent after the stop as a normal message, with its own reply, so
`bus agent interrupt AGENT --text "pause; reply with status"` is one call. The
result then includes its `message_id`; follow it with `wait`. When Claude Code
is interrupted before it answered, it puts the interrupted prompt back into its
input box; Bus clears it before typing the next message.

With AGENT, optional `--room ROOM` chooses the conversation for the receipt
and follow-up text; the interrupt notice stays in the target agent's room.
Without AGENT, `--room ROOM` stops every working or blocked agent in that room
except the `--as` agent, returning one entry per agent under `agents`; `--text`
then goes to every agent that stopped. The forms are:

```sh
bus agent interrupt AGENT [--room ROOM] [--text TEXT] [--as AGENT]
bus agent interrupt --room ROOM [--text TEXT] [--as AGENT]
```

Interrupting kills no process and keeps the provider context; to start a fresh
context, use `agent clear` once the agent is idle.

### Give an agent a fresh context

Start a fresh provider context in an idle agent's terminal before an unrelated
task:

```sh
bus agent clear "$agent_id"
```

Bus types the provider's reset command itself: `/clear` for Claude Code and
Codex, `/new-chat` for Cursor. It is not a Bus message, so nothing waits for a
reply. The agent keeps its terminal, name and room and rebinds to the new
provider session when the provider reports it: Claude Code and Codex with their
session start hook, Cursor with the first turn of the new chat. Its compaction
count starts again at zero. The agent must be idle with no message in progress
or queued. Do not send `/clear` or `/new-chat` with `send`: Bus would wait for a
reply that never comes, and would not rebind the agent.

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
bus state | jq '.result.build'
```

- `master_room` is the MASTER room's ID. `visible_room` is the room open in the
  UI, or `null` while a terminal or form is open instead. Deleting the visible
  room moves it to the room the UI falls back to, MASTER.
- Each room has `id`, `name`, `kind` (`master` or `work`), `notes`,
  `unread_count`, `unseen_to_developer`, `sound`, `sound_name`, `deletion_pending`, and
  `orchestrator`: the ID of
  the MASTER agent orchestrating it, or `null`, and `status`, the sidebar's
  status: `blocked` if any agent is blocked, on a dialog, or `unavailable`,
  else `working` if any agent works, else `idle`. An agent is `unavailable`
  while its provider is not running in its pane (it exited or crashed, or an
  updater or another program replaced it); deliveries to it stay queued with
  wait reason `agent_unavailable` until the provider is relaunched or resumed.
- `unread_count` counts incoming peer messages, replies and notices while a room is
  hidden. `unseen_to_developer` is the persisted sidebar green dot: it tracks
  messages addressed to You, including Bus notices, and clears when the room opens.
  Messages arriving in the visible room do not set either marker.
- Each agent includes `room_id`, `status`, `wait_reason`, `dialog`, `details_disclosed`,
  `orchestrates` (a MASTER agent's work room, `null` for work-room agents), and `compactions`:
  `count` and `last_at_ms` of the provider context compactions Bus observed for
  that agent. `dialog` is `true` while a numbered choice dialog waits for an
  answer; see [Answer an agent's dialog](#answer-an-agents-dialog).
  `wait_reason` says why a message queued for that agent would wait now (for
  example `prior_request_active`, `session_hook_missing` or
  `agent_unavailable`), or is `null` when nothing holds delivery back.
- `build` tells how the running Bus was built: `profile` is `debug` for a
  development build (`./run dev`, `cargo build`) or `release` for an optimized
  build, and `binary` is the running executable's path.
- `usage` has one entry per provider: `claude`, `codex`, and `cursor`. An
  `observed` entry reports `five_hour` and `weekly` windows with
  `used_percent`, `resets_at`, and `window_minutes`, plus `read_at_ms` and
  `observed_by_agent`. Codex usage is read after each Codex turn. Claude usage
  comes from its status-line payload, checked at most once per second. Bus
  preserves the configured status-line command, including claude-hud, with
  unchanged stdin and stdout and a two-second execution limit. User, project,
  and project-local status-line settings are captured when the agent launches;
  the same settings are reused on resume. Without a configured command, Bus
  shows a minimal status line. Managed settings can override the tap.
  Claude windows become `null` after their reported reset time, and observations
  expire after 15 minutes. Missing or expired data reports `unknown` with a
  `reason`; it never means the allowance is unused. After Bus restarts, Claude
  must emit a fresh observation. Cursor usage is not collected yet.
- `settings` holds the settings every Bus shares (see Shared settings below):
  `color_blind_mode`, `master_sound`, `room_sound` and `developer_sound`.

## Sound notifications

Bus can ding when a new message lands in a room, like a group chat. A room rings
for an agent's final reply or a message an agent sent with `send --as`, never
for the developer's own sends. Ordinary room sounds have a two-second startup/resume
grace period and one ding covers a burst. Mandatory developer alerts use their separate
system-sound selector and bypass room switches, startup grace and ordinary cooldown.

MASTER starts with sound on; work rooms start off. Toggle a room from Settings
in the UI. MASTER's sound is shared by every session (see Shared settings
below); a work room's sound belongs to its session.

Each room also picks which sound it plays: `Default`, Bus's own ding, or one of
the operating system's sounds. Bus lists them from `/System/Library/Sounds`,
`/Library/Sounds` and `~/Library/Sounds` on macOS, `%SystemRoot%\Media` on
Windows, and `/usr/share/sounds` (the freedesktop theme first) on Linux, each by
its file name without the extension. In Settings, Left and Right on a room, or a
click on `‹` or `›`, step through the sounds and play the new one as a preview.
`state` reports each room's `sound` and `sound_name`.

The `All rooms` row at the top of ROOMS in Settings sets every work room at
once: toggling it turns every room on or off, and changing its sound gives
every room that sound, leaving the other half of each room's choice alone. It
is also what new work rooms start with (`room_sound`, off with `Default` until
changed). Each room's own row below can still differ; while the rooms differ,
`All rooms` shows `[-]` or the sound `Mixed`, and toggling it turns every room
on.

Bus stores the name, not the path, and plays the sound once from the client
running the session. A sound that is no longer installed or fails to play
falls back to the default ding, which honors a custom `[ui.sound]` `path` or
`done_path`. `BUS_DISABLE_SOUND` silences all of them.

## Inspect rooms, replies, and terminals

Read the permanent message history for a room (including deleted agents):

```sh
bus history --room "$room_id"
```

Messages and requests are retained after agent/room deletion and provider context
clears, with no seven-day expiry. Deleted participants show `NAME [deleted]`.
`history --room ID` also reads a deleted room. Live room names resolve first;
deleted names are a fallback when unambiguous among deleted rooms.
`message status ID` continues to return archived requests and posted notices.
Storage and recovery limits are documented in the repository file
`docs/message-history.md`.

Each entry has the message (`prompt`: author, text, files, recipients and
`submitted_at_ms`) and its `delivery` status. Messages delivered to nobody,
such as a MASTER agent's report to the developer or an interrupt record, have
`delivery: null`.

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
Answer such a prompt as described in
[Answer an agent's dialog](#answer-an-agents-dialog). Use terminal output as
evidence for developer or model judgment, never as a substitute for message
settlement or an automatic workflow signal.

If current facts prove an idle agent still owns a historically wedged request,
the developer or an orchestrating agent can invoke the same queue-preserving typed
recovery after checking its identity and revision facts:

```sh
bus request recover "$request_id" --confirm
```

The generic form is `request recover REQUEST_ID --confirm`. For an untyped
queued request, it removes only that request, even if the agent is working,
blocked, launching, or unavailable. It preserves any current turn and other
queued work. Recovering an already abandoned request succeeds without replaying
it. A typed current request requires confirmed idle state, or an unavailable
agent whose session binding is invalidated (`session_invalidated`). Recovery
closes the current request and its joined group; for a joined request, recover
the lead request identified by its `group` field. It preserves other queued
work and does not reactivate the invalidated agent or replay typed work.
After session invalidation, replace the agent before resending. Recovery does
not undo effects of an interrupted task; inspect them before resending.

Bus also releases a request on its own when the agent never started it as a
turn. This happens when Bus typed the message while the agent ran a turn of its
own, and then the agent finished that turn and either started another turn or
stayed idle for five seconds. The request becomes `abandoned` (as `wait` and
`message status` report), and the next queued message is delivered. For a
work-room worker, its no-reply outcome also reaches the orchestrator terminal.
A later captured final for an orchestrator request is routed with its original
message reference; developer-authored replies keep their existing behavior.

The control CLI always emits raw JSON. Prompt and reply text returned by
`wait`, `message status`, and `history` remains raw Markdown. The interactive
room history renders Markdown for every message, prompts and agent replies
alike; a newline typed in a prompt stays a line break. Copying a message or
quoting a reply preserves its raw Markdown source.

Attached files and images appear as absolute paths: in each `history` prompt's
`files`, and in the message-level `files` of `message status` and `wait`. Images
pasted into the Bus composer are saved under `attachments/` in the Bus data
directory, so those paths stay readable after the session ends.

In the room history, image attachments (PNG, JPEG, GIF first frame, WebP) show
as the picture itself, up to eight rows tall, in place of their file name, when
the host terminal draws images: Kitty graphics in kitty, Ghostty, WezTerm and
iTerm2 3.7 or later, and the inline image protocol in older iTerm2 (iTerm2 is
detected by `TERM_PROGRAM=iTerm.app` or `LC_TERMINAL=iTerm2` and its version
variable). Kitty graphics move a picture in place while the history scrolls;
older iTerm2 redraws it on every scroll step. Not inside tmux, and not when
`terminal.kitty_graphics` is turned off. Clicking a picture opens the file
detail. Elsewhere (for example Terminal.app), or when the file is missing or
unreadable, a clickable `[file name]` row shows instead. A picture draws only
while it is fully in view and no dialog covers the history.

## Answer an agent's dialog

Agents run with their normal settings, so they stop at numbered choice dialogs:
permission prompts, folder-trust prompts, and question panels such as Claude
Code's `❯ 1. Yes / 2. Yes, and don't ask again / 3. No` or Codex's
`› 1. Yes, proceed (y)`. Bus finds any numbered option list on an agent's screen
and the selected option, marked by `›`, `>`, `❯` or similar, or drawn
highlighted. An unnumbered list counts too when an `Enter to ...` key hint
follows it, as in Claude Code's folder trust prompt (`❯ No, exit` above
`Yes, I trust this folder`); its options are numbered from the top. `bus state`
marks such an agent `dialog: true` and `status: blocked`, using the same status
as the agent list even when its provider still reports idle.

Once an agent has been blocked for about a second, on a dialog, a question, a trust
prompt or any other screen it cannot pass alone, Bus posts one notice in its
work room, signed `bus`: `bus → ORCHESTRATOR: AGENT. Blocked, needs
help to continue.` when the room has an orchestrator, delivered and waking it
like any message, or `bus → You: …` when it has none. The notice names the agent and includes `Inspect: bus agent dialog AGENT`,
but no options or fingerprint: look at the agent's terminal with that command (or
`bus agent read AGENT --source visible`) and answer as below. It is posted once
per dialog episode; selection changes and unreadable redraws do not repeat it.
When the dialog closes, Bus posts nothing more. A blocked MASTER orchestrator
instead gets a Bus developer-attention notice in MASTER, since MASTER has no
orchestrator of its own. Numbered text in an editable Claude or Codex native
composer is not a dialog and generates no notice.

`wait` stops early with the error code `agent_waiting_on_dialog` and the last
status when a recipient that has not replied shows a dialog or a blocked
screen; `message status` lists such recipients in `waiting_on_dialog`. Answer
the dialog, then wait again.

To answer, observe the dialog for a fresh fingerprint, decide, then choose
one option:

```sh
bus agent dialog "$agent_id"
bus agent choose "$agent_id" --option 2 --fingerprint "$fingerprint"
```

`agent dialog AGENT` returns `dialog` with `kind` (`choice` or `question`),
`text` (the question above the
options), `options` (`number`, `label`, `selected`), and `hint` (the key hint
below them), plus a `fingerprint`. Both are `null` when no dialog is visible.
Free-text questions have an empty `options` array, including a focused Claude
custom answer; use `agent answer` for text or to skip it.
It works while the agent is still launching, before its session starts.

`agent choose AGENT --option N --fingerprint FINGERPRINT` sends keys only while
the agent's launch, terminal, pane, and session (once bound) are unchanged and
its screen still shows exactly the observed dialog, including which option is
selected, checkbox state, and active question tab. For a native row it presses
Up or Down from the selected option to option N, then Enter; explicit question
navigation options send Left or Right alone. It never types digits or letter
shortcuts. A fingerprint is spent before
any key is sent, so it answers at most one dialog; observe again for a fresh
one. The result lists the `keys` sent and an
`outcome` from watching the screen for up to two seconds: `closed`, `replaced`
(another dialog appeared), `selection_moved` (the selection changed but the
dialog stayed), or `unchanged`. Nothing is sent when no dialog is visible, the
option does not exist, or the selected option cannot be seen.

Claude Code's `AskUserQuestion` checkbox panels use the same commands. API
option numbers follow the actual selectable rows, including Claude's
unnumbered `Submit` or `Next` button before `Chat about this`. Use the numbers
returned by `agent dialog`, rather than the numbers printed in the terminal:
with four checkboxes and a custom field, API option 6 is `Submit` and option 7
is `Chat about this`, even though Claude prints `6. Chat about this`.

1. Observe, choose a checkbox row to toggle it with Enter, then observe again.
   Read its `[ ]` or checked label to verify the result. Every toggle requires
   a fresh fingerprint; choosing a checkbox does not submit the form.
2. For a header such as `← ☐ Storage ☐ Region ✔ Submit →`, the options also
   include `Previous question` and `Next question`. Choosing either sends only
   Left or Right, keeping answers in their own tabs. `text` retains the tab
   header; `selected` marks the focused native row, not whether a checkbox is
   checked. Tab highlighting is included in fingerprint checks.
3. Choose `Next` to advance from a question or `Submit` to open Claude's
   review screen. Observe that new screen and choose `Submit answers` to finish,
   or `Cancel` to return and revise. The review requires its own fingerprint.

For example, when the observed labels show `6: Submit`, submit with
`bus agent choose "$agent_id" --option 6 --fingerprint "$fingerprint"`, then
run `bus agent dialog "$agent_id"` again before confirming `Submit answers`.

To submit a free-text answer or skip it, fetch a fresh fingerprint and use
exactly one of `--text` and `--skip`:

```sh
bus agent dialog "$agent_id"
bus agent answer "$agent_id" --text "MY TOKEN" --fingerprint "$fingerprint"
# Or, after fetching another fresh fingerprint:
bus agent answer "$agent_id" --skip --fingerprint "$fingerprint"
```

`agent answer` uses the same identity and single-use fingerprint checks as
`choose`, including the current input text. It pastes the literal text and
presses Enter after the short confirmation delay; `--skip` sends Codex's
Ctrl+] or Cursor's Esc. Claude questions refuse `--skip`
(`skip_would_cancel_form`): Esc there declines the whole question or form. Its result includes `keys`, `skipped`, and
the same `outcome` values, with `input_changed` for a question whose text field
changed while it remained open. A numbered choice cannot be answered as text,
except through its free-text row (below).

Codex's expanded `Queued follow-up inputs` / `Type your answer` form is
supported; its collapsed question banner remains a working state while the
agent is running. Expand that banner in the native pane with Shift+Left.
Claude Code's `AskUserQuestion` option list ends with a `Type something.` row,
which `agent dialog` marks `"free_text": true` (multi-select forms: the
`[ ] Type something` checkbox) and summarizes as `free_text` (`option`,
`draft`, `checkbox`). Answer it in one step with
`agent answer --text` on the choice dialog's fingerprint: Bus moves to the row,
types the text and presses Enter for a single-select field. On a multi-select
field, typing ticks the checkbox; Bus sends no Enter, which would untick it.
`agent choose` refuses an empty single-select field, because Enter there declines
the whole question. If someone left a draft
in the row, `agent answer` refuses (`free_text_holds_draft`) and `agent choose`
on the row submits the draft; replacing a draft needs the agent's pane.
`agent answer --text` refuses existing drafts (`free_text_holds_draft`) for every
provider, including focused Codex `Other` and Cursor fields; edit or submit those
drafts in the agent's pane. In a
short pane Claude shows only part of the list: `agent dialog` returns the
visible options with their real numbers, `choose` also takes the numbers
scrolled out of view above or below the window, and `free_text.option` names
the row even when hidden. `answer --text` then moves to it first and types
only once the redrawn field shows it empty (a draft there gets no text,
`free_text_holds_draft`). After a free-text answer on a multi-select question,
the ticked field keeps focus; it stays a choice, so choose Next or Submit.
When the cursor is already on the row, `agent dialog` shows it as a focused
text question, as it does for Cursor's `Other` row.
A multi-question form (`←  ☐ Fresh files  ☐ Helper home  ✔ Submit  →`) shows
one question at a time; `agent dialog` adds `tabs`, each with `label`,
`answered` and `active`. Answer the active question with `choose` or, for its
free-text row, `answer --text`; Claude then moves to the next question. When
every question is answered, the Submit tab offers `Submit answers` and
`Cancel`; choose `Submit answers`. `--skip` is refused for Claude questions
(`skip_would_cancel_form`): Esc declines the whole form, not one question. Cursor's checkbox chooser
still needs its own navigation adapter.

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
2. Capture room, message and request IDs. Use globally unique current agent
   names for readability; numeric or historical names include the sender ID
   when needed to address that identity unambiguously.
3. Use `--to all` only when broadcasting is intentional.
4. Use `wait` when a program needs completion; do not treat `send` as completion.
   Orchestrators receive terminal event notifications without periodic polling.
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

### Resume developer work

Use `bus sessions` to identify a saved session, then resume its UI with its
exact ID. After resuming, control commands target it as the last opened
session.

## Edit or clean up

Replace a room's notes, the free-text box under the room name in the UI. The
text replaces the whole field; pass `--text ""` to clear it. `state` returns each
room's current notes. Only work rooms have notes: MASTER shows no notes box, and
`room notes` on MASTER fails with "The MASTER room has no notes":

```sh
bus room notes "$room_id" --text "Goal: ship notes
Non-goals: UI changes"
```

`state` reports whether an agent's details are shown in the sidebar
(`details_disclosed`) and the UI's color-blind palette
(`settings.color_blind_mode`); both are toggles in the UI.

### Shared settings

Settings that are not tied to one room are shared by every Bus session:
color-blind mode, MASTER's sound (`master_sound`), the last `All rooms` sound
which new work rooms start with (`room_sound`), and the developer alert's system
sound name (`developer_sound`, null for System default) and its on/off checkbox
(`developer_sound_enabled`, default true). They live in `settings.json` beside the session
registry (`~/.local/share/bus/settings.json`). A change made in any Bus, from
the UI or the CLI, is what every Bus launched or resumed afterwards starts with:
at launch MASTER takes `master_sound`, and each room created later takes
`room_sound`. Bus instances already running keep their MASTER sound until they
restart. `settings` prints the shared file; `state` reports what each room
actually uses:

```sh
bus settings
```

`master_sound` is `null` until a Bus first launches with shared sound settings;
that launch keeps its session's MASTER sound and records it. Old sessions and
old settings files load unchanged.

An explicit `BUS_DATA_DIR` root without a registry session (tests, e2e runs,
isolated development copies) keeps its own `settings.json` in that root and
never reads or writes the shared file.

Rename rooms and agents, or delete resources explicitly:

```sh
bus room rename "$room_id" "verification"
bus agent rename "$agent_id" "Code Reviewer"
bus agent delete "$agent_id" --confirm
bus room delete "$room_id" --confirm
```

Deleting a room or agent is destructive. Resolve the target with `state`, prefer
its numeric ID, and pass `--confirm` only after checking it.

Stop the session's server and every agent pane it hosts:

```sh
bus stop
```

`stop` targets the same session as control commands (`BUS_DATA_DIR`, else the
last opened session). It waits until the server is gone and prints
`{"stopped":true}`, or `{"stopped":false}` when no server was running. An
attached UI loses its server and exits without saving drafts, so quit the UI
with Ctrl+Q instead, which saves first and then stops the server itself.
A destructive command without `--confirm` fails and names the missing flag.

Deletion closes an agent's terminal only while the running server still
attributes it to that agent. After its provider exited, Bus has usually closed
that terminal already (see Provider exits); otherwise deletion closes the shell
the provider left, even if it is running a command. When the server refuses
that close (another agent or managed name took the terminal, it is shared with
another pane, or its session would not stop), Bus still retires the agent, preserves its messages and requests, leaves the terminal open, and lists it under
`terminals_left_open` in the result. Close it yourself if it is no longer needed.

A successful delete response means the agent was removed durably and that
removal was published to the UI snapshot. If terminal close cannot be confirmed
or persistence fails, the command returns an error; inspect `state` for
`deletion_pending` and `actionable_error` before retrying. A control timeout
does not cancel deletion: it may still finish. Reuse the same `--request-id`
to retrieve its retained outcome rather than assume it failed.

## Upgrading to the always-on control socket

Every session now answers control commands. A Bus that is still running from an
older build answers them only if it was launched in developer mode; after
its next restart, every session does.

## Upgrading from the orchestrator build

Sessions saved by the build that had the built-in room orchestrator open
normally, with these changes:

- Each session gains the MASTER room. A work room already named MASTER, in any
  case, is renamed once to `Master (old)` and keeps all its data.
- A room's Room Brief goal and non-goals are appended once to its notes.
- The orchestrator's own transcript, workflow drafts, and capability grants are
  dropped, because those features no longer exist.
- `<root>/private/orchestrator-credentials.json` is no longer read. Delete it
  yourself if you no longer need the API key it holds.

## End-to-end check

`tools/acceptance/e2e_test.py` checks message round trips against real Claude Code,
Codex and Cursor agents. It spends real model usage, so it runs only with
`--allow-live-models`, which `just e2e` passes for you:

```sh
just e2e
just e2e --providers claude,codex --cases single,resume
```

Each run starts its own Bus from `target/debug/bus` (or `--binary PATH`)
in a pseudo-terminal with a fresh owner-only
`BUS_DATA_DIR` under `temp/e2e/<timestamp>/`, after removing every inherited
`BUS_*`, `HERDR_*` and `CLAUDE_CODE_*` variable, so it never touches another
Bus. Retained pane/session compatibility keys are scrubbed too.
It drives that Bus only through these control commands, and the agents
work in a scratch repository it recreates at `temp/e2e/workspace`. Folder
trust and update prompts are answered with `agent dialog` and `agent choose`.
At the end it quits Bus, stops its server, and kills any process still holding
the run's directories. `--keep` leaves the data directory for debugging.

| Case | Checks |
| --- | --- |
| `single` | The reply equals the token and the request settles with no agent error. |
| `queued` | Three messages sent at once all settle, replied in order. |
| `multi` | One message to every selected provider. |
| `background` | Claude ends a turn with a background shell running; the reply is recorded and the next message still delivers. |
| `dialog` | The agent raises a dialog mid-turn; it is answered through Bus and the reply lands. |
| `orchestrator` | A MASTER orchestrator of its own work room, with the default prompt, settles developer messages: plain, with an attached image, steered mid-turn, after a background shell (Claude), and once more after. |
| `adoption` | The orchestrator is deleted and re-added with `--resume` of its session; the same round trips settle in that same session. Needs `orchestrator`. |
| `resume` | Bus quits and restarts on the same data directory; each agent relaunches into its session and a new round trip works. |

`--providers` defaults to every installed provider CLI and `--cases` to all of
them. A case a provider cannot run is reported `SKIP` with the reason: the
background case is Claude-only, and the dialog case skips when the provider's
own settings approve the command without asking (Cursor with Run Everything).
Claude raises its dialog with a question, because its auto mode approves
commands by itself and Bus refuses permission launch args. The run prints a
table of every provider and case with its result and time, writes
`temp/e2e/<timestamp>/report.json` with the commands and last status of each
failure, and exits non-zero when any case fails.

Run `just e2e` before merging changes that touch delivery, callbacks or launch.

## Diagnose failures

Start with the durable control state:

```sh
bus state
bus message status "$message_id"
bus history --room "$room_id"
bus agent read "$agent_id" --source visible
```

`state` reports each agent's status, `wait_reason`, `actionable_error`,
runtime identity and current request. `message status` reports each request's
`stage` and `reason`, and `agent read` shows what the agent's terminal shows.

Common failure patterns:

- **Control is unavailable:** confirm the intended Bus process is still running
  (`bus sessions`), then verify every terminal uses the same
  `BUS_DATA_DIR` when an override is present.
- **A selector is ambiguous:** rerun `state` and use the numeric room or agent
  ID instead of its name.
- **Agent setup needs consent:** inspect the reported project hook path, then
  use `--consent-hooks` or `agent setup-confirm ... --confirm` only if intended.
- **A request remains queued:** inspect its `reason` and the agent's
  `wait_reason` in `state`; the agent may be busy, blocked, or have an
  actionable error.
- **A request remains `awaiting_start`:** submission alone did not establish a
  trusted provider turn. Inspect `state` and recent agent output before
  deciding whether to retry.
- **`wait` times out:** retain its returned last status and continue with
  `message status`; a timeout does not prove failure or authorize duplicate
  work.
- **Agent terminal identity changed:** stop using `agent read` for that selector
  and inspect the owned session before taking action.

For the current command list and keyboard shortcuts, run `bus --help`.
