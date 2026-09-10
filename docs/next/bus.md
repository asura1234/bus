# Bus

Bus is a local, standalone terminal fork of Herdr for a human coordinating
selected coding agents. Rooms show each agent's latest final response. Open an
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
- Click notes or press F3 to edit them. Notes are ordinary text and are not
  automatically sent to agents. F3 again returns to the composer.
  The notes editor is boxed beneath the room name, with a horizontal separator
  immediately below it before chat history, without an empty spacer row.
- Type `@` (Shift+2 on a US keyboard), or click the recipient control, to open a checkbox menu. All applies
  only to the current room. Arrows move; Space or Enter toggles; Escape closes.
  Mentions inside pasted or quoted text remain literal.
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
- Quote appends the reply to the current local draft, escapes embedded quotes
  and backslashes, preserves multiline text and recipients, and focuses the
  composer. It does not send.
- Click the disclosure beside an agent's provider to show its actual branch
  and PWD. Click its name for the real terminal. Room unread counts accumulate
  while an agent terminal, another room or a form is open.
- Use the mouse wheel or trackpad over the left column to scroll ROOMS and
  AGENTS together, separated by a matching-color horizontal line. All room
  names, agents and expanded details remain reachable. Scroll over chat history
  to browse its messages independently of the fixed composer; scroll over the
  draft to browse its text. Both sidebar and history stop at their boundaries
  instead of scrolling into empty space. Runtime updates continue in every view.

The sidebar shows runtime status. Queue and error notices remain separate from
reply cards. Existing replies stay visible until a correlated new final reply
arrives. Unavailable or invalidated sessions are shown as such, not as an idle
or completed agent. A changed provider session requires creating a new Bus
agent; existing request ownership remains preserved.

## Files and local editing

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
Use the sidebar's Review hook setup action only after reviewing all listed
hooks and closing the CLI's setup menus. Codex emits SessionStart with its
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
