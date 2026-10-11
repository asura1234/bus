# bus --dev tui: drive a scratch Bus through its real terminal

`bus --dev tui` starts a fresh Bus in a hidden terminal and lets you read its
screen as text, type, press keys, click, drag, scroll, resize and wait for
conditions. It is test tooling: only the dev variant (cargo feature
`dev-tools`, as `./run dev` builds) contains it, and it runs only behind
`--dev`. A release build (`./run prod`, `just build`, npm) refuses `tui`.

In the Bus repository, run `./run tui VERB …`. It builds the dev variant of
your working tree into `target/tui-driver` (or `$CARGO_TARGET_DIR/tui-driver`)
and runs `--dev tui` from there, never touching a `debug` binary that
`./run dev` builds. Everywhere else, run `bus --dev tui VERB …` from a
dev-variant build other than the developer's `target/debug/bus`, which
`start` refuses (cargo's own tests opt in with `BUS_TUI_ALLOW_TARGET_DEBUG=1`).

Every session is a new Bus the driver starts itself. That Bus gets its own
generated data directory under `/tmp/bus-tui/NAME/` (`$BUS_TUI_ROOT/NAME/`
when that is set to an absolute path, as the driver's own tests do), its own
server and its own terminal. No verb attaches to an existing session. `start`
claims NAME atomically and its host refuses to run for a session it does not
own, so a second start or host never touches a running one. `start` refuses a
target that a live Bus already serves, refuses when an inherited `BUS_*` or
`HERDR_*` variable points at the target, and hands the scratch Bus none of
the inherited Bus variables.

The scratch Bus finds fake `claude`, `codex` and `cursor-agent` CLIs first on
its PATH, so agents never start a real provider. The fake Claude fires Bus's
real hooks and answers each prompt with `fake reply: <prompt>`. Like Claude
Code, it submits on Enter (CR), takes a typed LF as a newline, shows each line
that is one quoted image path as a leading `[Image #N]`, and clears a line on
Ctrl+K or Ctrl+U. A prompt containing `fake:work N` keeps it Working for N
seconds first.

The same PATH holds recording stand-ins for `open`, `osascript`,
`terminal-notifier` and `afplay`, the commands Bus uses on macOS to open links
and files, show desktop notifications, read a clipboard image and play sounds.
They record each call (see `effects`) and do nothing else: nothing opens, no
notification appears, no sound plays, and Ctrl+V pastes the session's fixture
image (`fixture clipboard-image`), never your clipboard. Sessions therefore run
on macOS only: on Linux and Windows Bus uses other programs the recorder does
not stand in for, so `start` refuses there.

What notifies in Bus: the room UI never shows a desktop or terminal
notification. A new agent reply (or agent notice) in a room whose sound is on
plays that room's sound (`afplay`); MASTER's sound starts on, work rooms' off.
Host focus does not matter: `focus out` neither causes nor suppresses it.
Desktop notifications exist only for agent-pane toasts, which are off by
default and not shown inside the Bus room UI.

Output: every verb prints one JSON line on stdout, except `journey` (one
line per step, then the result), `bus` (Bus's own output) and
`snapshot --plain`. A failure also prints `error: CODE message` on stderr.

Exit codes:

- 0: ok
- 1: an assertion failed, a wait timed out, or a target was not found
- 2: usage error (run the verb with `--help`)
- 3: no session, or the Bus under it died

After Bus exits (Ctrl+Q, a crash), `status`, `snapshot`, `find`, `cells`,
`screenshot`, `effects` and `clipboard` still read the last screen and the
records, and `stop` still cleans up; every other verb exits 3.

Sessions: every verb takes `--name NAME` (default `s1`, or `$BUS_TUI_SESSION`).
At most 4 sessions run at once.

Coordinates are zero-based: `ROW COL`, row 0 is the top line, column 0 the
left edge. Prefer `--text` targets to raw coordinates.

Artifacts: each session writes a run folder with `trace.ndjson` (every
request and its result), `pty.cast` (asciicast of the terminal, input and
output), `pty.raw` (the exact output bytes), `shots/` (screenshots) and, after
`stop`, the Bus logs and `effects.ndjson` (every recorded effect). Inside a Bus
checkout it lives under `temp/tui-driver/runs/`, otherwise under
`~/.local/state/bus/tui-runs/`.

Verbs: start, stop, status, list, bus, snapshot, find, cells, screenshot,
hits, type, paste, press, click, hover, drag, scroll, resize, focus, wait,
expect, effects, clipboard, fixture, journey, gc.

Targets: click, hover, scroll and `wait --style` take a cell `ROW COL`, a
visible text `--text TEXT [--regex] [--nth N] [--rows A-B]`, or a named
region of the room UI `--hit NAME` (see `hits`). Prefer `--hit`, then `--text`.
Run `bus --dev tui VERB --help` for one verb.

### start
`start [--name NAME] [--size COLSxROWS] [--run-dir DIR]`

Starts a fresh scratch Bus (default size 160x45) and waits until it answers
`bus state` and has drawn its first room. Prints the session status with
`ready_ms`. A relative `--run-dir` is taken from the current directory.
Refused when the session name is running or still starting, when 4 sessions
run, when the target belongs to a live Bus, when inherited
`BUS_*`/`HERDR_*` variables point at it, or when the binary is
`target/debug/bus`. A start that fails stops whatever its Bus started.

### stop
`stop [--name NAME]`

Quits the scratch Bus, stops its server, terminates processes the session
started that still hold its directory (never anything else, including the
caller), copies the Bus logs and `effects.ndjson` into the run folder and
removes the session directory. Prints the run folder. If the effect log cannot
be copied, `stop` fails with `effects_not_kept` and leaves the session directory
(and the log in it) until the next `start` of that session.

### status
`status [--name NAME]`

Session, process ids, whether Bus is alive, size, frame count, data and run
folders.

### list
`list`

Running driver sessions.

### bus
`bus [--name NAME] COMMAND [ARGS…]`

Runs a Bus control command against the session's scratch Bus only, for
example `bus state`, `bus room create review` or `bus agent add --room
review --name a1 --provider claude --pwd /tmp/bus-tui/s1/work`. Use it to
set up state; drive the behaviour under test with real input.

### snapshot
`snapshot [--rows A-B] [--plain]`

The screen as numbered text rows (`  7│ text`), plus the cursor, size and
frame `seq`. `--plain` prints just the rows, for reading; it is not
available as a journey step. A frame Bus is still drawing is never shown:
every screen read sees the last complete frame.

### find
`find TEXT [--regex] [--rows A-B]`

Every visible match, with its row, column and width in cells.

### cells
`cells ROW COL [WIDTH HEIGHT]`

Cells with their symbol, `fg`/`bg` colours (`#rrggbb`, `idx:N` or
`default`), attributes (bold, dim, italic, underline, inverse, …) and
hyperlink. Use it for colour questions.

### screenshot
`screenshot [--real] [--out FILE]`

Writes a PNG of the screen and prints its path; open it to look. By default
it draws the cells (text, colours, attributes, borders) with a bundled font:
use it for layout, overlap and colour. It does not draw inline images.

`--real` replays the session's exact output into a fresh Ghostty window the
driver starts with default settings, checks the window shows the session's
whole grid, captures only that window by its id and closes it. Use it to see
what a real terminal shows, inline images (Kitty graphics) included. It needs
macOS, Ghostty in /Applications and Screen Recording permission for the
terminal running the driver; a window briefly appears. The replay leaves out
clipboard writes, notifications and bells.

### hits
`hits`

The room UI's clickable regions as of its last frame, from the dev-tier
`bus ui hits`: each with a name, its cells and the text it covers. Names are
stable: `room:NAME`, `agent:NAME`, `new-room`, `new-agent`, `settings`,
`color-blind`, `sound:ROOM`, `composer`, `recipients`, `recipient:NAME`,
`files`, `file:NAME`, `remove-file:NAME`, `field:N`, `provider:claude`,
`suggestion:N`, `quote:ID`, `add`, `cancel`, `delete-room:NAME`, and more.
A region covered by a later one is clicked where it is still visible.

### type
`type [--] TEXT`

Types text as keystrokes. Put `--` before text that starts with `--`.

### paste
`paste [--] TEXT`

Pastes text, bracketed when the client enabled bracketed paste. Put `--`
before text that starts with `--`.

### press
`press KEY [KEY …]`

Presses keys in order: `enter`, `esc`, `tab`, `shift+tab`, `backspace`,
`up`, `down`, `left`, `right`, `pageup`, `pagedown`, `home`, `end`,
`delete`, `insert`, `f1`…`f12`, `space`, letters, and chords such
as `ctrl+n`, `ctrl+shift+r`, `alt+up`. Keys are encoded the way Bus's
terminal negotiated, so Esc never waits out a timeout.

### click
`click (ROW COL | --text TEXT [--regex] [--nth N] [--rows A-B]) [--double] [--button left|middle|right] [--mods shift,alt,ctrl]`

Clicks a cell, or the middle of a visible text match. A text that matches
more than once needs `--nth N` or `--rows`. `--double` sends two clicks
inside Bus's double-click window.

### hover
`hover (ROW COL | --text TEXT | --hit NAME)`

Moves the pointer onto a target with no button down, as a mouse does before
a click, so tooltips and hover details show.

### drag
`drag ROW1 COL1 ROW2 COL2 | --from-text A --to-text B [--button B] [--mods M]`

Presses at the first cell, moves, releases at the second. When Bus copies
the selection, the copied text is in `copied` (Bus copies with OSC 52 in
driver sessions, so your clipboard is never touched).

### scroll
`scroll (ROW COL | --text TEXT) (--up N | --down N) [--mods M]`

Wheel notches at a cell.

### focus
`focus in|out`

Tells Bus its terminal gained or lost focus (`CSI I` / `CSI O`), as a real
terminal does when you switch windows.

### resize
`resize COLSxROWS`

Resizes the terminal; Bus relays out.

### wait
`wait --text TEXT [--regex] [--gone] [--rows A-B] [--timeout 10s]`
`wait --stable 300ms [--rows A-B] [--cols A-B] [--timeout 10s]`
`wait --changed [--rows A-B] [--cols A-B] [--timeout 10s]`
`wait --style TARGET [--fg C] [--bg C] [--attrs a,b] [--no-attrs a,b] [--timeout 10s]`
`wait --effect KIND [--target TEXT] [--all] [--timeout 10s]`
`wait --state 'PATH OP VALUE' [--timeout 10s]`
`wait --message ID [--timeout 10s]`

Waits for text to appear (or with `--gone`, to disappear); for a region to
stay unchanged (`--stable`) or to differ from how it was just before the
last input (`--changed`), comparing text and colours; for every non-blank
cell of a target to have a style; for an effect recorded since the last input
(`--all`: ever); for `bus state` to match; or for a message to settle. Exit 1
on timeout, with what it saw; exit 3 when the scratch Bus exits during the
wait (a dead Bus's last screen is never "stable", and an unreadable state
never matches). Never sleep instead. Scope `--stable` and
`--changed` to the region you care about: agent spinners and toasts keep the
whole screen moving.

Colours are `#rrggbb`, `idx:N` or `default`, as `cells` prints them;
attributes are bold, dim, italic, underline, inverse, strike, blink, hidden.
Effect kinds are listed under `effects`.

State predicates: PATH is dotted into the `bus state` result (`visible_room`,
`rooms.2.name`, `agents.*.status`; `*` means any element). OP is `==`, `!=`,
`contains`, `>`, `>=`, `<` or `<=`. VALUE is JSON, or a bare word taken as a
string, for example `agents.*.status == idle`.

### expect
`expect …`

The same as `wait`, read as an assertion.

### effects
`effects [--kind KIND]`

What the scratch Bus asked the operating system to do, oldest first, each
with an index and a time: `open` (a URL or file; `target` is what it named),
`notification` (title in `target`, `body`), `sound` (the file played),
`clipboard_image` (an image paste read; `served` says whether a fixture was
there), `osascript` (other scripts), and `terminal_notification` (OSC 9, 99
or 777 in Bus's output). `since_last_input` is the index the next
`wait --effect` starts from.

### clipboard
`clipboard`

Every copy Bus made in this session (OSC 52).

### fixture
`fixture image|file|clipboard-image [--file FILE] [--text TEXT] [--pattern P]`

Writes a file for the session to send and prints its path: `image` is a
160x100 PNG easy to recognise in a screenshot, by `--pattern`: `quadrants`
(default: red, blue, yellow and cyan, clockwise from the top left),
`red-diagonal` (a red band from top left to bottom right on white) or
`blue-circle` (a blue disc on white); `file` is a text file. Both go into the
session's work directory. `clipboard-image` sets the image that Ctrl+V in the
composer pastes.

### journey
`journey STEPS.json`

Runs steps in one session and stops at the first failure. The file is a JSON
list of `{"verb": "press", "args": ["ctrl+n"], "pause_ms": 0}`. `start`,
`stop`, `bus` and `journey` are not allowed as steps.

### gc
`gc [--older-than 7d] [--run-dir DIR]`

Deletes old run folders; folders of running sessions are kept.
