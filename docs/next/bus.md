# Bus

Bus is a local, standalone terminal fork of Herdr for a human coordinating
selected coding agents. Rooms retain prompts and their final responses in chronological history. Open an
agent to interact with its actual CLI, then click a room or press F6 to return.
Agent names are editable aliases, not roles or permission assignments.

## Run from this checkout

Build once, then run `./bus`:

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools \
ZIG=/Users/dylanliu/work/bus/temp/tools/zig-aarch64-macos-0.15.2/zig \
cargo build --locked --bin herdr
./bus
```

The build command above is the verified local macOS setup. Other hosts need
their normal Rust toolchain and the project's supported Zig 0.15.2.
`./bus --help` lists shortcuts; `./bus --paths` prints the selected directories.
The launcher uses this checkout's built native executable, not a browser.

Bus stores its room state under `~/.local/share/bus`. Override this with an
absolute `BUS_DATA_DIR` for an independent test instance:

```sh
BUS_DATA_DIR=/absolute/path/to/bus-test ./bus
```

Both Herdr config and session state live inside that root, in `herdr-config`
and `herdr-state`. Bus selects its own `bus` session, clears inherited Herdr
socket/config overrides at launch, and leaves HOME and XDG variables unchanged.
Normal `herdr` startup without the Bus setting is unchanged. Bus disables
upstream onboarding, update checks and sounds. Starting Bus does not install
provider integrations or start a model. The first room is empty except for an
editable reminder about coordination goals and non-goals.

Only one Bus coordinator may own a data root. Closing the client leaves the
native server and agent terminals available; callback records can spool until
Bus reconnects. Run the launcher again with the same data root to reconnect.

## Developer logs

Start with `./bus --dev` to enable DEBUG/TRACE diagnostics in log files, not in
the terminal UI. `./bus --dev --paths` prints their locations without starting a
session. Ordinary `./bus` keeps normal INFO logging; dev mode keeps three rotated
backups per log (5 MiB each). Raw keyboard/paste and notification-content dumps
are excluded even in dev mode. Delivery logs contain IDs, states, byte counts,
error codes and timing, not prompt/reply text, attachment contents or API keys.

- Client/coordinator: `~/.local/share/bus/herdr-config/sessions/bus/herdr-client.log`
- Native terminal server: same directory, `herdr-server.log`
- Provider hooks: `~/.local/share/bus/callbacks/<launch-id>/hook.log`

Paths follow `BUS_DATA_DIR`. New servers and their newly launched agents inherit
dev logging. An already-running server or agent keeps its original environment;
Bus shows a warning rather than restarting it. Client delivery diagnostics still
work. Use the explicit server restart below **only when safe to stop the agents**
if you need full new native-server diagnostics, and reopen with `./bus --dev`
instead of `./bus` so the replacement server inherits verbose logging.
Diagnostics never grant hook
trust, release queued messages, resend uncertain submissions or restart agents.

Follow `bus.message.submit` → `bus.message.queued` → `bus.delivery.start` →
`bus.api.start/result` → `bus.terminal.queued/result` → `bus.callback.spooled` →
`bus.callback.correlated` → `bus.reply.persisted` → `bus.reply.received`.
The command ID connects UI submission to queueing; request, prompt, room and
agent IDs connect the round trip. The API request ID connects the client call
to the native server, and callback ID/sequence connects hook capture to consume.
`bus.terminal.result` confirms the terminal writer outcome, **not** model
acceptance; the trusted start callback provides that evidence. `bus.reply.received`
means the room UI received a snapshot, not that it rendered a particular frame.

`bus.delivery.wait` records why a queued request cannot be sent (for example,
`hook_setup_unconfirmed`, `agent_blocked` or `prior_request_active`), once per
reason change. Rejected/uncertain sends, mismatched callbacks, missing companion
hooks, storage failures and status transitions have separate events. Normal
callback spool files may be acknowledged and removed; diagnostic breadcrumbs
remain until log rotation. Logs are local diagnostic data: inspect them before
sharing, since upstream runtime logs can still include local paths and errors.

After a build that adds server features (including guarded terminal deletion),
the already-running native server still uses its older code. Missing features
report an error; Bus never falls back to an unguarded terminal close. To use the
new server, quit Bus and, **when you are ready to stop all its running agent
sessions**, run the following from this checkout, then reopen Bus:

```sh
BUS_DATA_DIR="${BUS_DATA_DIR:-$HOME/.local/share/bus}" ./target/debug/herdr session stop bus
./bus
```

Use the same `BUS_DATA_DIR` you normally use. This is an explicit server restart,
not required for ordinary client-only UI changes. Room data and project files
remain on disk; existing agent processes are stopped.

## Working in rooms

- Click Rooms `+`, or press Ctrl+R, to add a room. Each room has independent
  notes, recipients, files and draft text.
- Type `/help` in the composer and press Enter to open the local shortcut
  guide. It is never sent to agents, and selected recipients and files remain
  unchanged. Page Up/Down or the mouse wheel scroll the guide; Esc or Enter
  closes it. Shortcuts are not permanently displayed along the bottom.
- Click Agents `+`, or press Ctrl+N, to add an agent. Enter its name, select
  Codex, Claude Code or Cursor, choose a PWD and optionally add supported launch
  arguments. PWD belongs to each agent, not the room: agents in one room can
  work in separate frontend and backend projects. PWD suggestions browse
  actual local directories. Arrows select; Tab or a click completes.
  Enter adds the agent from any field; Esc cancels (also for room creation).
  After completion, another Tab moves to the
  next field; typing a prefix or pressing an arrow browses further.
- Double-click a room or agent name to rename it. F2 renames the currently
  selected room or terminal agent. Enter saves; Escape cancels.
- Click `×` beside a room or agent name to delete it. A warning popup names
  the target; **Cancel (Esc)** leaves it unchanged and **OK (Enter)** confirms.
  Deleting an agent stops its terminal session and removes its Bus replies and
  pending requests. Deleting a room does this for every agent in the room and
  removes the room's notes, messages and draft. Project directories, files and
  shared provider hook configuration are not deleted. Once confirmed, stopping
  cannot be undone. If a session cannot be safely stopped, deletion stays
  pending with delivery suspended; the warning gives a short explanation.
  Enter or Esc dismisses the failure without retrying. Start deletion again
  explicitly after resolving the problem. An older server that cannot safely
  close a session must be updated; Bus never falls back to an unguarded close.
  Detailed failures are in developer logs, not the popup.
  Other rooms and agents remain unchanged. Deleting the
  last room leaves an empty room list, including after restart.
- Click notes or press F3 to edit them. Notes are ordinary text and are not
  automatically sent to agents. F3 again returns to the composer.
  The notes editor is boxed beneath the room name, with a horizontal separator
  immediately below it before chat history, without an empty spacer row.
- Type `@` (Shift+2 on a US keyboard), or click the recipient control, to open a checkbox menu. All applies
  only to the current room. Arrows move; Space or Enter toggles; Escape closes.
  Mentions inside pasted or quoted text remain literal.
  Selected names appear in rounded chips in the composer. Whole chips wrap
  onto additional rows; a name too wide for the pane is shortened with an
  ellipsis, with its full name available in the picker. If the chip rows exceed
  the screen, scroll over them to reach the rest.
- Enter sends to the selected agents. Shift+Enter inserts a newline when the
  host terminal reports it distinctly. Ctrl+J is the newline fallback; legacy
  terminals may encode Shift+Enter exactly like Enter. Bracketed paste and text
  composition events insert text and never submit it.
  Checked recipients stay selected after sending until you change or clear them.
  With no checked agents, Enter shows an error and preserves the draft.
  `/help` is the only command accepted without recipients; it opens locally.
- The composer grows with newlines and wrapped text up to the full room pane
  height, then scrolls. Ctrl+E toggles full-height/compact editing without
  changing the draft or recipients; a cleared/sent draft returns to automatic
  sizing. Page Up / Page Down scroll the draft (Fn+Up / Fn+Down on many Mac
  keyboards), as does the mouse wheel over the input. Scrolling does not move
  the insertion point; typing or moving the caret brings it back into view.
  The recipient picker and file chips stay available at full height. F3
  temporarily makes room for notes without losing the draft's size setting.
  A visible box encloses the recipient controls, draft and files. Its border
  and the sidebar divider use the same accent color, as do the notes box and
  section separators. A horizontal line below the +/@ bar separates its
  controls from the editable draft.
  The composer sits flush with the bottom; a footer row is reserved only while
  an error or status message is visible.
- Quote appends the specific clicked reply (including older replies) to the current local draft, escapes embedded quotes
  and backslashes, preserves multiline text and recipients, and focuses the
  composer. It does not send.
- Click the disclosure beside an agent's provider to show its actual branch
  and PWD. Click its name for the real terminal. Room unread counts accumulate
  while an agent terminal, another room or a form is open.
- Use the mouse wheel or trackpad over the left column to scroll ROOMS and
  AGENTS together, separated by a matching-color horizontal line. All room
  names, agents and expanded details remain reachable. Scroll over chat history
  to browse every saved prompt and completed reply independently of the fixed
  room header, notes, and composer. History opens at the bottom with the latest
  messages; new arrivals do not pull you away while reading older messages.
  Scroll back to the bottom to follow new arrivals again. Scroll over the
  draft to browse its text. Both sidebar and history stop at their boundaries
  instead of scrolling into empty space. Runtime updates continue in every view.

The sidebar shows runtime status. Queue and error notices remain separate from
reply cards. Older replies remain in history when a new correlated final reply
arrives. Unavailable or invalidated sessions are shown as such, not as an idle
or completed agent. A changed provider session requires creating a new Bus
agent; existing request ownership remains preserved.

## Files and local editing

### Message identity and time

Outgoing `You → recipients` headers use the green accent. Each agent has a
persisted identity color used for its sidebar name, recipient chip, and reply
header. Creation chooses the candidate with the largest minimum Oklab distance
from green and the existing room colors. Candidates come from a deterministic
17-step RGB grid with at least 4.5:1 text contrast against the dark background;
recognizably green hues are excluded. This maximizes separation within that
candidate pool, not over the entire continuous color gamut. Colors stay stable
when agents are renamed or neighbors deleted. Existing agents get colors on
load without losing their saved conversations.

Both outgoing and incoming headers show the message time in the computer's
local timezone, in `HH:mm` 24-hour format. Messages older than 24 elapsed hours
show `> 1 day`, regardless of whether the calendar date changed.

### Attachments

Type `+` (Shift+= on a US keyboard), click the composer `+`, or press Ctrl+F,
to browse actual local files. These symbols stay literal in pasted/quoted
text, notes and file/path inputs. Select a
directory to complete it; select a file to attach its absolute path. Paths are
revalidated by the coordinator. Relative paths and directories are rejected.
Two files with the same basename remain distinct by full path; duplicate paths
are deduplicated. Filename chips can be removed; hover exposes their paths when
the host reports pointer movement. Sent-file chips expose a path on click too.
A failed attachment is marked `!` and can be removed before retrying a send.

Terminal file drops are host-generated path pastes. In the focused composer,
an all-path paste such as `'/tmp/one file.md' /tmp/two.md` adds references without
replacing text, selecting recipients or sending. Ordinary text paste stays text.
The terminal cannot receive browser-style drag hover or hidden file metadata;
there is no invented filename-to-path mapping. Hosts should enable bracketed
paste. Unbracketed text ending with Enter is indistinguishable from typing and
pressing Enter; use the `+` picker when the host cannot provide paste framing.
The actual agent terminal retains its ordinary native paste and key behavior.

Bus sends text plus quoted absolute file paths, not uploads or file contents.
Agent file-access permissions continue to apply. Local text editors support
Unicode, arrows, Home/End, Backspace and Delete. Drafts remain locally owned
while saves are pending, and a send waits for the exact successful draft,
recipient and file acknowledgements. Save failures cancel the send rather than
submitting old text. Ctrl+C (or Ctrl+Q) quits Bus from any view, draining saves
and waiting for coordinator shutdown instead of forwarding an interrupt to
an agent. This exits the Bus client, not the agents' persistent sessions.
If saving fails or exceeds five seconds, Bus stays open and displays an
explicit retry/force-exit notice. Ctrl+Shift+Q then forces exit with possible
loss of unsaved edits if the host can report that chord. Abrupt process or
terminal termination cannot provide the clean-quit guarantee.

## Provider setup and safeguards

Provider executables and directories are validated before launch. Codex and
Cursor may need project-local observation hooks. Bus first shows the exact
path and hook notice; Add explicitly consents to writing only those owned
entries. This is separate from reviewing/trusting hooks in the actual CLI.
Use the sidebar's **Confirm setup** action after reviewing all listed
hooks and closing the CLI's setup menus. In Codex, inspect them using `/hooks`.
The **Confirm setup (Enter)** button is Bus's separate delivery confirmation,
not permission approval in the provider. Until it is confirmed, prompts remain
queued even if the terminal is idle. Codex emits SessionStart with its
first prompt: Bus sends that first prompt only to the exact ready managed
launch, then binds the actual session and turn before accepting a reply.
Subsequent sends require that bound session. No priming prompt or restart is
needed. Claude uses private launch settings and needs its real session-start
callback before the first send; Cursor likewise waits for session-start.
Codex's transcript-less title/memory background hooks are not room replies.

Additional arguments intentionally support only these interactive options:

- All providers: `--model VALUE` or `--model=VALUE`.
- Codex: `-m VALUE`, `-mVALUE`, `-m=VALUE`, `--no-alt-screen`, `--strict-config`,
  `--oss`, `--search`, and `--local-provider ollama|lmstudio`.
- Claude: `--verbose`, `--ax-screen-reader`, `--system-prompt VALUE`,
  `--append-system-prompt VALUE`, and `--effort low|medium|high|xhigh|max`.
- Cursor: `--plan` and `--mode plan|ask`.

Unknown options, subcommands and permission/configuration overrides are
rejected. Bus does not automatically approve permission menus, provision
worktrees, route conversations between workers, or run handoff/review pipelines.
Use ordinary prompts and local files for handoffs. Independent sessions are
not a filesystem sandbox.

## Dev-only control interface

Start the normal terminal app with `bus --dev`. A second terminal or coding
agent can then drive **that running coordinator** through JSON-producing CLI
commands. There is no separate headless coordinator or alternative delivery
implementation. Without `--dev`, Bus opens no control listener. Client commands
never start an instance, enable a listener, or restart an existing server.

```sh
bus state
bus room create review
bus agent add --room review --name codex1 --provider codex --pwd /absolute/project
bus agent setup-confirm codex1 --confirm
bus send --room review --to codex1 --text "Review the plan" --file /absolute/plan.md
bus message status 42
bus wait --message 42 --timeout 180
bus history --room review
bus agent read codex1
bus diagnostics
bus agent delete codex1 --confirm
bus room delete review --confirm
```

Use the message ID returned by `send`, not the example `42`. `--to` accepts
comma-separated names/IDs; `--to all` explicitly selects every agent in that
room. Names must resolve uniquely; use numeric IDs when names are ambiguous.
Room deletion stops and removes its agents, using the same guarded close as
the UI. `--consent-hooks` on `agent add` explicitly authorizes Bus's project
hook setup; `agent setup-confirm --confirm` records the separate Bus readiness
confirmation. Neither option grants provider permissions or trusts hooks on
the provider's behalf. Review actual provider setup before confirming.

`send` atomically preserves the human's composer draft. Its receipt means
**queued**, not delivered. `message status` reports each target separately:
`queued`, `submitting`, `awaiting_start`, `delivered`, or `replied`, with blocker
reasons, provider correlation IDs and the exact request's final reply.
`delivered` requires a trusted provider-start callback. A blocked permission
menu remains visible in agent status; only a completed, correlated final reply
counts as `replied`. History includes older prompts and their own replies.

Every command returns one JSON envelope with `id`, `ok`, `result`, and `error`;
failures exit nonzero. `wait` defaults to 60 seconds (maximum 600), exits on a
deadline, and preserves the last status for diagnosis. Mutations never retry
automatically. Supply `--request-id YOUR_UNIQUE_ID` to correlate logs and safely
repeat an uncertain command with exactly the same parameters during the same
coordinator lifetime. Receipts are **not persisted across restarts**. The cache
rejects new mutations at 256 receipts or an 8 MiB reservation limit rather than
evicting old receipts. Read/status/wait commands remain available. After a
restart or uncertain result, inspect state/history before issuing a new mutation.

The endpoint is private local IPC at `BUS_DATA_DIR/dev-control.sock`. The
default root is `~/.local/share/bus`; client and running app must use the same
root. Local control requires a private, owned data directory, and exposes no
network listener, generic native-method forwarding, or arbitrary keystroke
command. Treat returned history/terminal output as sensitive; dev logs contain
correlation metadata rather than prompt/reply bodies. `diagnostics` identifies
the client/server logs and per-launch callback logs.

For repeatable real-provider testing, start a separate normal dev Bus with a
disposable data root, then run:

```sh
BUS_DATA_DIR=/absolute/disposable/bus-data bus --dev
# In another terminal, after reviewing provider trust/setup for the test PWDs:
BUS_DATA_DIR=/absolute/disposable/bus-data python3 scripts/bus_dev_acceptance.py \
  --allow-live-models --claude-pwd /absolute/trusted/claude-project \
  --codex-pwd /absolute/trusted/codex-project
```

This creates two Claude and two Codex sessions and sends 20 unique prompts to
1/2/3/all targets. It checks real correlated replies, actual owned-terminal
output for target/non-target isolation, and chronological persisted history.
Evidence is saved under `temp/artifacts/bus-dev-*`; the test room remains for
inspection. It uses normal provider authentication and consumes model usage.
It does not inject callbacks, operate UI controls, or bypass permission menus.
This proves domain round trips, not rendered geometry or touchpad gestures.

For one Claude, one Codex and one Cursor (`cursor-agent`) with 15 prompts, use
the mixed-provider profile:

```sh
BUS_DATA_DIR=/absolute/disposable/bus-data python3 scripts/bus_dev_acceptance.py \
  --allow-live-models --profile claude-codex-cursor \
  --claude-pwd /absolute/trusted/claude-project \
  --codex-pwd /absolute/trusted/codex-project \
  --cursor-pwd /absolute/trusted/cursor-project
```

This covers every agent alone, every pair, and all three. Cursor uses its real
`sessionStart`, `beforeSubmitPrompt`, `afterAgentResponse`, and completed `stop`
callbacks; it is never substituted with another provider. The driver does not
close the app or delete the room on success or failure. Run Bus in the terminal
you want to inspect, then run the driver from a second terminal; leave the first
window open to check scrolling afterwards.

## Verification scope

Focused tests cover room/editor behavior, command acknowledgement and failure
ordering, Unicode edits, recipient routing, path paste, quote, inline rename,
file recovery, dropdowns, stale suggestions, native resize/input/cursor reuse,
pending-focus protection, display control-character filtering, and clean quit.
The no-model PTY smoke driver is
`temp/artifacts/bus-native-smoke.py`; it keeps captured native text/ANSI frames.
Its optional synthetic CLI mode uses disposable fixture hooks, provider-shaped
callbacks and real Herdr detector/PTY behavior. It is not evidence of an actual
vendor permission/trust flow.

The opt-in real-provider test is `scripts/bus_live_integration.py`. It launches
the actual native Bus UI in an isolated data root with the installed Claude,
Codex and `cursor-agent` CLIs, uses their normal authentication, and creates disposable Git
projects. It never injects synthetic callbacks or bypasses vendor trust.

```sh
python3 scripts/bus_live_integration.py --allow-live-models
```

The driver accepts JSON commands, one per line. Add each provider with
`{"op":"add","provider":"codex","name":"codex-live"}` (and `claude` /
`claude-live`, or `cursor` / `cursor-live`). Cursor uses the exact `cursor-agent`
executable, not a generic `agent` command. Inspect `{"op":"frame"}` and resolve the displayed project
and hook setup using `send` or `click`; no dialog is approved automatically.
Use `{"op":"case","names":["codex-live"]}` to select through the real `@`
checkbox menu and send a unique harmless prompt through composer Enter. Repeat
for Claude and Cursor alone, repeat each send, and select multiple agents together. Once replies arrive, use
`{"op":"verify","token":"THE_RETURNED_TOKEN"}`. This checks completed
request ownership, trusted session/turn binding, exact final text, and each
agent's rendered reply card independently of the submitted prompt. `stop`
closes only the disposable test session. Captures and evidence remain in
`temp/artifacts/bus-live-*`.

Real local first-send and repeat-send results are recorded in
`temp/artifacts/bus-live-integration-2026-09-10.md` and
`temp/artifacts/bus-cursor-live-integration-2026-09-10.md`. These do not establish
every mid-turn vendor permission-menu variant or other operating systems.

Bus preserves Herdr's upstream license and attribution in `LICENSE` and the
existing repository documents. No replacement terminal framework is used.
