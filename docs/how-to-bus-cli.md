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

The PWD works as for any agent; Bus writes nothing into it. Every MASTER agent
launches with an orchestrator system prompt, by default Bus's built-in one
(`src/bus/prompts/orchestrator.md` in the Bus repository). Replace it with
`--system-prompt TEXT` or `--system-prompt-file PATH`. Bus fills in
`{{ROOM_NAME}}`, `{{ROOM_ID}}`, `{{AGENT_NAME}}` and `{{DOCS}}`, the folder
`<BUS_DATA_DIR>/docs/` where Bus writes the docs the prompt links to: this
guide, the orchestrator guide, `workflow-create.md`, the workflow template and
example workflows.

Bus delivers the prompt with each provider's own launch option and passes it
again on resume:

| Provider | Delivery |
| --- | --- |
| Claude Code | `--append-system-prompt-file`, added to Claude Code's default prompt |
| Codex | `-c developer_instructions=...`, a developer message beside Codex's base instructions that points Codex at the prompt file, so the launch command Bus types stays short |
| Cursor | No launch option exists, so Bus sends the prompt as the agent's first message |

The prompt is kept in the launch's callback folder as `system-prompt.md`. A new
PWD can show the provider's "trust this folder" prompt on first launch; answer
it in the agent's terminal. Bus does not pre-trust folders.

Reassign or unassign it later:

```sh
bus agent orchestrate "$orchestrator_id" --room "$other_room_id"
bus agent orchestrate "$orchestrator_id" --none
```

The system prompt is fixed at launch, so Bus sends the orchestrator a message
naming its new room, or saying it has none.

Additional launch args (`--args`, or the form's Args field) work for MASTER
agents exactly as in any room and combine with the Bus-owned prompt arguments.

### Move an existing session into MASTER

An agent can adopt an existing provider session instead of starting a new one,
for example to turn a Claude Code conversation into an orchestrator. Pass the
session's ID (a UUID) in the launch args, in the provider's resume form:

| Provider | Launch args | Where to find the ID |
| --- | --- | --- |
| Claude Code | `--resume SESSION_ID` | `/status` in the session |
| Codex | `resume SESSION_ID` (first) | `/status` in the session |
| Cursor | `--resume SESSION_ID` | `cursor-agent ls` |

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
  conversation otherwise replays the system prompt it started with. A resumed
  Codex thread keeps its original developer instructions, so Bus sends the
  prompt as its first message, as for Cursor.

The generic forms are `agent add --room master ... [--orchestrates ROOM]
[--system-prompt TEXT | --system-prompt-file PATH]` and
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
`send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...] [--as AGENT] [--queue]`.

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
| `joined` | The message joined another message's turn; see `group`. |
| `replied` | Bus recorded the final reply for that recipient. |

Treat `complete: true` from `message status` or `wait` as the settlement signal.
A successful terminal write, a visually idle agent, or a queued focus change is
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
the message stays queued and the dialog notice tells you what the agent needs.
A message sent to an idle agent starts a new turn and group, as before. When
messages piled up while the agent could not take them (it was launching,
blocked, or finishing another turn), Bus sends them as one prompt, in order,
each part headed `[N/M from SENDER at HH:MM]`, and they form one group.

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
  `unread_count`, `sound`, `sound_name`, `deletion_pending`, and
  `orchestrator`: the ID of
  the MASTER agent orchestrating it, or `null`.
- Each agent includes `room_id`, `status`, `dialog`, `details_disclosed`,
  `orchestrates` (the work room it orchestrates, or `null`), and `compactions`:
  `count` and `last_at_ms` of the provider context compactions Bus observed for
  that agent. `dialog` is `true` while a numbered choice dialog waits for an
  answer; see [Answer an agent's dialog](#answer-an-agents-dialog).
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
  `color_blind_mode`, `master_sound` and `room_sound`.

## Sound notifications

Bus can ding when a new message lands in a room, like a group chat. A room rings
for an agent's final reply or a message an agent sent with `send --as`, never
for the human's own sends. Nothing rings for the first two seconds after start
or resume, and one ding covers a burst of messages.

MASTER starts with sound on; work rooms start off. Toggle a room from Settings
in the UI or with the CLI. MASTER's sound is shared by every session (see Shared
settings below); a work room's sound belongs to its session:

```sh
bus room sound "$room_id" --on
bus room sound master --off
```

Each room also picks which sound it plays: `Default`, Bus's own ding, or one of
the operating system's sounds. Bus lists them from `/System/Library/Sounds`,
`/Library/Sounds` and `~/Library/Sounds` on macOS, `%SystemRoot%\Media` on
Windows, and `/usr/share/sounds` (the freedesktop theme first) on Linux, each by
its file name without the extension. In Settings, Left and Right on a room, or a
click on `‹` or `›`, step through the sounds and play the new one as a preview.
From the CLI:

```sh
bus sounds
bus room sound "$room_id" --on --sound Glass
bus room sound master --on --sound Default
```

The generic form is `room sound ROOM (--on | --off) [--sound NAME]`; names
match case-insensitively, and an unknown name changes nothing. Without
`--sound`, the room keeps its sound. `sounds` lists every choice with its
`name` and `path` (`null` for `Default`). `state` reports each room's `sound`
and `sound_name`.

New work rooms start with the shared new-room sound, off with `Default` until
changed. Settings shows it as the `New rooms` row at the top of ROOMS; the CLI
sets it with `settings room-sound (--on | --off) [--sound NAME]`, which takes
names as `room sound` does and keeps the sound without `--sound`. Changing it
leaves existing rooms as they are:

```sh
bus settings room-sound --on --sound Glass
```

Bus stores the name, not the path, and plays the sound once from the client
running the session. A sound that is no longer installed or fails to play
falls back to the default ding, which honors a custom `[ui.sound]` `path` or
`done_path`. `HERDR_DISABLE_SOUND` silences all of them.

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
Answer such a prompt as described in
[Answer an agent's dialog](#answer-an-agents-dialog). Use terminal output as
evidence for human or model judgment, never as a substitute for message
settlement or an automatic workflow signal.

If current facts prove an idle agent still owns a historically wedged request,
the Human or an orchestrating agent can invoke the same queue-preserving typed
recovery after checking its identity and revision facts:

```sh
bus request recover "$request_id" --confirm
```

The generic form is `request recover REQUEST_ID --confirm`. It abandons only the
exact confirmed current request and does not choose what happens to queued work.

Bus also releases a request on its own when the agent never started it as a
turn. This happens when Bus typed the message while the agent ran a turn of its
own, and then the agent finished that turn and either started another turn or
stayed idle for five seconds. The request becomes `abandoned`, a Bus notice in
the room says so, and the next queued message is delivered. Any answer the agent
gave is in its terminal.

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
marks such an agent `dialog: true`.

Bus tells someone without being asked. Once a dialog has been on screen for
about a second, Bus sends the room's orchestrator a message in MASTER, delivered
like any other message: the agent and room, the question or complete command
(including every command line and its `Reason:`), and the numbered options
with the selected one marked. Key hints such as `Press
enter to confirm or esc to cancel` are left out. The answer line is `bus agent
dialog AGENT`, then `bus agent choose AGENT --option N` for choices, or
`bus agent answer AGENT --text "..."` / `bus agent answer AGENT --skip` for
free-text questions; the fingerprint is
not in the message, because `agent dialog` fetches a fresh one. A room without
an orchestrator gets the same text as a Bus notice in the room itself, for the
Human. Each dialog is reported once, a blocked screen without a readable
dialog is reported with the `agent read` command to inspect it, and when that
dialog closes Bus adds one line, `answered: option N` when Bus recorded the
choice, otherwise `answered`.

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
Free-text questions have an empty `options` array.
It works while the agent is still launching, before its session starts.

`agent choose AGENT --option N --fingerprint FINGERPRINT` sends keys only while
the agent's launch, terminal, pane, and session (once bound) are unchanged and
its screen still shows exactly the observed dialog, including which option is
selected. It presses Up or Down from the selected option to option N, then
Enter; it never types digits or letter shortcuts. A fingerprint is spent before
any key is sent, so it answers at most one dialog; observe again for a fresh
one. The result lists the `keys` sent and an
`outcome` from watching the screen for up to two seconds: `closed`, `replaced`
(another dialog appeared), `selection_moved` (the selection changed but the
dialog stayed), or `unchanged`. Nothing is sent when no dialog is visible, the
option does not exist, or the selected option cannot be seen.

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
Ctrl+] or the other providers' Esc. Its result includes `keys`, `skipped`, and
the same `outcome` values, with `input_changed` for a question whose text field
changed while it remained open. The closing notice is `answered`, with no
option number. A numbered choice cannot be answered as text.

Codex's expanded `Queued follow-up inputs` / `Type your answer` form is
supported; its collapsed question banner remains a working state while the
agent is running. Expand that banner in the native pane with Shift+Left.
Claude Code's `AskUserQuestion` and Cursor's questions also support text
answers when their custom input is focused: move to Claude's `Type something.`
row or Cursor's `Other` row in the native pane, then use `agent dialog` and
`agent answer`. Moving to a custom field requires native pane navigation;
`choose` confirms a numbered option with Enter, and Cursor's checkbox chooser
still needs its own navigation adapter. Unfocused custom fields are not
treated as text questions.

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

### Shared settings

Settings that are not tied to one room are shared by every Bus session:
color-blind mode, MASTER's sound (`master_sound`) and the sound new work rooms
start with (`room_sound`). They live in `settings.json` beside the session
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
unsent drafts and exits. It does not wait for the exit. The session's server and
its agents keep running so `bus resume` can reattach to them.

Stop the session's server and every agent pane it hosts:

```sh
bus stop
```

`stop` targets the same session as control commands (`BUS_DATA_DIR`, else the
last opened session) and works without `--dev`. It waits until the server is
gone and prints `{"stopped":true}`, or `{"stopped":false}` when no server was
running. An attached UI loses its server and exits without saving drafts, so
run `bus quit` (or press Ctrl+Q) first, as the end-to-end check does.
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
  dropped, because those features no longer exist.
- `<root>/private/orchestrator-credentials.json` is no longer read. Delete it
  yourself if you no longer need the API key it holds.

## End-to-end check

`scripts/bus_e2e.py` checks message round trips against real Claude Code,
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
Bus. It drives that Bus only through these control commands, and the agents
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
| `orchestrator` | A MASTER orchestrator of its own work room, with the default prompt, settles human messages: plain, with an attached image, steered mid-turn, after a background shell (Claude), and once more after. |
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
