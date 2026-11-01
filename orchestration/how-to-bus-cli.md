# How to use the Bus CLI

Bus coordinates native coding-agent terminals. The UI is for conversations;
control commands drive an already-running session. Bus stores delivery and replies; callers decide what
those replies mean and which task comes next.

## Getting help

Run `bus <cmd> --help` for behavior, flags/defaults, JSON fields, errors, and
examples. Nested commands use `bus agent add --help`, for example. `bus --help`
lists commands; a bare group also lists its children. Help needs no session.
Control commands emit one JSON envelope; require `ok: true` and capture IDs.
Local session commands describe their own text/JSON output in their help.
Read the [orchestrator guide](orchestrator-guide.md) for running a team, the
[binding rules](orchestrator-rules.md) for authority, and the
[Bus communication guide](bus-communication-guide.md) for message routing.

## Start or resume Bus

A plain `bus` launch creates a new local session. Keep its terminal open; run
control commands from another terminal or an agent pane. They never start Bus.
Use `bus sessions`, then `bus resume SESSION_ID` or `bus resume --last` to
resume. `bus --paths` prints data/log/config/state locations without launching.
See `bus resume --help` for hooks, provider sessions, and environment restrictions.

## Target the intended running session
By default, control commands target the last opened session. For concurrent
sessions, automation and tests, launch and control the same dedicated root:

```sh
export BUS_DATA_DIR=/absolute/path/to/my-bus-session
bus                       # UI terminal
# In a second terminal with the same BUS_DATA_DIR:
bus state
```

The override is an exact isolated root; relative paths are rejected. Missing
roots are created owner-only (0700) for the private control socket. An isolated
root without a registry session keeps its own settings.json, too.
Never use a live session as a test fixture. Scrub inherited Bus/Herdr environment
and use a scratch root. Session launch/log flags are documented by `bus --help`.
`--yolo` bypasses provider permission prompts for new/resumed workers and MASTER
orchestrators for this run only; it defaults off and is never saved.

## Rooms, agents, messages and turns

Every session has exactly one MASTER room. It contains orchestrators; each
orchestrates exactly one work room for life, and each work room has at most one.
Deleting a work room deletes its orchestrator. MASTER cannot be renamed/deleted.
Use `bus agent add --help` for `agent add --room master ... --orchestrates ROOM`,
prompt replacement/adoption, providers, hook consent and launch restrictions.

Rooms are conversations; agents have home rooms and globally unique current
names/IDs. Any agent can message any other agent across rooms. `--room` selects
the conversation, never a namespace. Capture numeric IDs when identity must stay
stable through renames. `bus agent list` maps names, IDs and home rooms.
On loading old duplicate names, the lowest ID keeps its name; later duplicates
become NAME-ID (with a suffix if needed). This is recorded once before clients
connect, preserving sessions, room bindings and queued work.

A message has one ID and one recipient request per selected agent. The request
tracks delivery and its answer; a provider turn is the agent's execution context.
A same-room message can steer into a running turn and join its group; `--queue`
gets an independent idle turn. A cross-room message waits for the next turn.
Explicit correlated replies complete individual requests without ending the turn.
An eventual final answers only the group's still-outstanding requests.

A reply lands in the message's room, equally for developer and agent senders.
A turn no message started posts in the orchestrator's work room. A captured Stop
answer ends its matching requests immediately even if background shells keep
Working; later background results are follow-ups and never replace that answer.

### Who to message

Use the [Bus communication guide](bus-communication-guide.md). Send only the body;
do not add a "from X", "X here" or signature line. Bus adds `name:` locally or
`name (room R):` across rooms/MASTER; numeric/historical names include an ID.
Agent-authored batches add position/time without repeating the sender header.
New agents get protocol instructions at launch; upgrades do not insert task turns
into existing conversations with saved instructions.
#### Reach the developer from any room

For a developer-only request from any room:

```sh
bus send --room ROOM --as AGENT --to you --text '[waiting-on-you] Approve access.'
# An orchestrator can report in MASTER (human is the compatibility alias):
bus send --room master --as claude-orch --to human --text 'Merged PR #N.'
```

`@you` in prose means the developer; use `--to you` in the CLI. The message
stays in its conversation, with no agent request to wait for. Only hard blockers
use `[waiting-on-you]` (or legacy `[waiting_on_you]`), which also sends a Feishu
DM. Agent posts to You and explicit @you/blocker markers alert the TUI; peer
traffic saying merely “waiting on you” does not page the developer. See
[developer notifications](../docs/developer-notifications.md) for setup, burst
format and deduplication. Developer sends never notify.

## Send work and wait for the reply
```sh
room_id=$(bus room create feature-work | jq -r '.result.room_id')
agent_id=$(bus agent add --room "$room_id" --name worker --provider codex \
  --pwd /absolute/path/to/project | jq -r '.result.agent_id')
message_id=$(bus send --room "$room_id" --to "$agent_id" \
  --text 'Inspect the change; report evidence and risks.' | jq -r '.result.message_id')
bus wait --message "$message_id" --timeout 600
bus history --room "$room_id" --limit 100
```

`send` proves durable queueing, not start or completion. A terminal write or a
visually idle pane also proves neither. Use the same persisted message status
through `wait` or `message status`; it survives when the
waiting CLI process exits or Bus restarts. `complete: true` is the durable settlement signal.
Bus does not interpret the reply, infer dependencies or add a workflow polling
loop. Sequence dependent implementation/review tasks in the caller. Keep external
side effects explicit in the task. There is no `send --wait`; use send then wait.

For independent investigation, send to a comma-separated recipient list and
wait on the shared message ID; inspect each request separately. Use `all` only
for intentional room broadcasts. A worker's ordinary messages use `--as`.
When answering work, run `bus send --reply-to MESSAGE_ID --as YOUR_AGENT_ID
--text ANSWER` in the foreground and check success before claiming delivery.
See `bus send --help` for first-answer conflicts and uncertain retry handling.

### Async waits and settlement

`send --async` sends and follows without a deadline, waiting through dialogs.
`wait --async` follows an existing ID but stops for a dialog. Bounded waits
need replies; async waits also accept abandonment specifically as reply_not_captured.
Read `bus wait --help` and `bus message status --help` for polling, stages,
outcome windows, provider failures, retained last status and exit codes.
A timeout does not cancel work or authorize a duplicate send. For an uncertain
submission, inspect transcript and composer before resending; effects may exist.
See [agent launch arguments](../docs/launch-args.md#interactive-provider-launch) for command input and launch failure behavior.

Orchestrators run async sends as background tool calls (Claude Code Bash:
run_in_background; Cursor: background shell with timeout 0), then inspect the
combined output when woken. Codex cannot wake on background command completion;
new Codex MASTER orchestrators are refused, but saved ones still resume.

Worker events wake the work-room orchestrator through ordinary agent messages.
A wake is only the one real notice: an explicit result not already delivered, or
an actionable final outcome (startup failure, session limit, unsent submission).
Working means the request is in progress; a stuck screen is Blocked; a dead launch
is Unavailable; an Idle agent without a captured reply gets one final outcome.
Agents are only Idle, Working, Blocked or Unavailable. Results already in the room
are not repeated, and Bus does not wrap results or post process-exit messages to
MASTER. Worker-to-worker replies reach their author only. Interrupt/delete outcomes
do not generate notices. There are no idle-worker/request nudges or room monitors.

Replies and captured finals reach any agent requester through a live waiter or
ordinary messages. Waiters defer only their own wakes and mark only flushed output seen;
use `message seen` only after reading. A killed waiter loses its lease, resuming
normal queued delivery with composer guards. Deleted requesters are not replaced.
stdout and native injection can expose an already-delivered wake.

Event decisions, pending sources and batches persist together. Repeated callbacks,
polls and restarts are not new events; later outcomes/turns are. Definite rejection
retains the queue ID and pauses retries after its queued outcome window until new
work/identity/status/restart, not idle polls. Uncertain submissions are never
replayed. Payloads freeze once native submission starts. Failed acknowledgements
preserve fallback. First upgrade backfills retained eligible replies/outcomes once;
expired history stays expired, while rollback discarding routing state can repeat recovery.

## Diagnose, recover and answer dialogs

Start with `state`, `message status ID`, and `history --room ROOM`. Inspect a
terminal only when those leave a question: `agent read AGENT --source visible`
or `agent read AGENT --source recent --lines N`. Output is evidence for judgment,
never a workflow signal. A failed identity check discards text: inspect ownership.
For provider exits, inspect state.exit_snapshot and its saved file; the provider
terminal may already be closed. Exit snapshots preserve viewport/200 recent lines;
notices preview 20 nonblank lines (160 characters each, 1600 total). Work-room
terminals close after a saved snapshot; MASTER terminals stay open. A failed
snapshot leaves the terminal open. Quoted blocker markers cannot page the developer.
Unrequested post-startup exits stay in lifecycle state and logs; MASTER does not
receive a process-exit chat message. A later send gets the saved exit outcome.
Update shared provider installations separately while their agents are stopped.

Blocked agents generate one room notice after about a second, addressed to its
orchestrator or otherwise You. Blocked MASTER orchestrators notify You in MASTER.
Selection changes/unreadable redraws do not repeat it; closed dialogs post nothing.
The orchestrator's blocked notice can steer across rooms and takes priority.
After two minutes unsubmitted/unconfirmed it falls back to the developer; a
readable closed dialog suppresses fallback. Unreadable state gets at most three
extra minutes before fallback; uncertain writes are never replayed.

Observe with `agent dialog AGENT`, then use `agent choose AGENT --option N
--fingerprint FINGERPRINT` or `agent answer --help`. Every action needs a fresh
observation and a decision; do not automate by guessing from terminal text.
Read the help for checkbox forms, custom text, drafts and tab navigation.
Fix provider installation/startup first, then `agent resume --room ROOM` for
saved agents. Use `request recover REQUEST_ID --confirm` only after checking
identity/status and effects; it closes work, never undoes or replays it.
Use `agent interrupt --help` to stop a turn; `agent clear --help` for fresh context.
Sending provider reset slash commands as messages leaves requests unanswered.

## UI, settings and cleanup

The UI renders Markdown; copying/quoting keeps raw source. Pasted images live in
session attachments/ as absolute paths.
History shows PNG/JPEG/GIF-first-frame/WebP images up to eight rows tall when
fully visible without a dialog: Kitty graphics in kitty/Ghostty/WezTerm/iTerm2
3.7+, inline images in older iTerm2 (TERM_PROGRAM or LC_TERMINAL and version).
Kitty moves images with scrolling; older iTerm2 redraws them. tmux, disabled terminal.kitty_graphics,
missing/unreadable images and unsupported hosts use clickable file rows. Clicking an image opens file detail.

Enter steers; Option/Alt+Enter queues. Ctrl+J or supported Shift+Enter adds a
newline. `bus --help` lists other keyboard shortcuts. Settings exposes details
visibility and color-blind mode; `bus settings --help` explains shared defaults.
Room sound rings for agent finals/posts, never developer sends. MASTER defaults
on, work rooms off; startup/resume has a two-second grace and bursts ring once.
YOU alerts have a separate enabled-by-default sound selector and bypass ordinary
grace/cooldown/room switches; turning its checkbox off silences only that audio.
System default prefers Glass then another installed sound; missing selections fall back.
Settings Left/Right or ‹/› previews sounds. All rooms changes every work room and future defaults,
preserving each switch/name's other half; differing rooms show [-]/Mixed, and toggling turns all on. Sounds come from system/user sound folders
(macOS /System/Library/Sounds, /Library/Sounds, ~/Library/Sounds; Windows
%SystemRoot%\Media; Linux /usr/share/sounds, freedesktop first). Missing/failed
sounds fall back to Bus's ding honoring ui.sound path/done_path. BUS_DISABLE_SOUND
silences audio. See state/settings help for effective versus saved values.

Resolve deletion targets with state, prefer IDs, then use --confirm. Read each
delete command's help for pending deletion and terminals left open. Quit the UI
with Ctrl+Q to save drafts before stopping; `bus stop` disconnects without saving.
A failed or five-second save shows an unsaved warning: Ctrl+Q again within five seconds forces quit,
potentially losing edits. Wait out that window to retry saving; presses before a warning keep waiting.

Sessions from pre-control builds answer control only if launched in developer mode;
after restart every session does. Older orchestrator-build sessions gain MASTER (a conflicting old room becomes Master (old)); brief goal/
non-goals append once to notes. Obsolete orchestrator transcripts/workflow drafts/
capability grants are dropped. private/orchestrator-credentials.json is no longer
read; delete it yourself if its old API key is no longer needed.

## Developer restart for orchestrators

`bus --dev restart --binary PATH --confirm` replaces the running server and keeps this session. Without `--dev` it is refused and hidden from `bus --help`.
Before it, every room's orchestrator asks its working agents to pause: commit and push, stop builds and tests, release slot locks, then stop.
Wait for each confirmation, then restart. After the outcome notice, tell them to resume.
Details: `bus --dev restart --help`.

## Repository status and acceptance
Repository status reporting belongs to the agent/harness; Bus never polls hosts
or decides release steps. Follow the communication guide for meaningful transitions.
Workers report routine status to their orchestrator, never the developer; a missing
orchestrator or failed send is not permission to send routine status to the developer.
For real-provider round trips use `just e2e` (explicit live-model usage), or
`just e2e --providers claude,codex --cases single,resume`. The harness
(tools/acceptance/e2e_test.py --help) documents providers/cases/artifacts/cleanup.
Run `just e2e` before merging delivery, callback or launch changes.
