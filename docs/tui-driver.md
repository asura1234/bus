# bus --dev tui: drive a scratch Bus through its real terminal

`bus --dev tui` starts a fresh Bus in a hidden terminal and lets you read its
screen as text, type, press keys, click, drag, scroll, resize and wait for
conditions. It is a dev tool: it works only behind `--dev`.

In the Bus repository, run `./run tui VERB …`. It builds your working tree
into `target/tui-driver/bus` and runs `--dev tui` from there, never touching
`target/debug`. Everywhere else, run `bus --dev tui VERB …`.

Every session is a new Bus the driver starts itself. That Bus gets its own
generated data directory under `/tmp/bus-tui/NAME/`, its own server and its
own terminal. No verb attaches to an existing session. `start` refuses a
target that a live Bus already serves, refuses when an inherited `BUS_*` or
`HERDR_*` variable points at the target, and hands the scratch Bus none of
the inherited Bus variables.

The scratch Bus finds fake `claude`, `codex` and `cursor-agent` CLIs first on
its PATH, so agents never start a real provider. The fake Claude fires Bus's
real hooks and answers each prompt with `fake reply: <prompt>`. A prompt
containing `fake:work N` keeps it Working for N seconds first.

Output: every verb prints one JSON line on stdout. A failure also prints
`error: CODE message` on stderr.

Exit codes:

- 0: ok
- 1: an assertion failed, a wait timed out, or a target was not found
- 2: usage error (run the verb with `--help`)
- 3: no session, or the Bus under it died

Sessions: every verb takes `--name NAME` (default `s1`, or `$BUS_TUI_SESSION`).
At most 4 sessions run at once.

Coordinates are zero-based: `ROW COL`, row 0 is the top line, column 0 the
left edge. Prefer `--text` targets to raw coordinates.

Artifacts: each session writes a run folder with `trace.ndjson` (every
request and its result), `pty.cast` (asciicast of the terminal, input and
output) and, after `stop`, the Bus logs. Inside a Bus checkout it lives under
`temp/tui-driver/runs/`, otherwise under `~/.local/state/bus/tui-runs/`.

Verbs: start, stop, status, list, bus, snapshot, find, cells, type, paste,
press, click, drag, scroll, resize, wait, expect, clipboard, journey, gc.
Run `bus --dev tui VERB --help` for one verb.

### start
`start [--name NAME] [--size COLSxROWS] [--run-dir DIR]`

Starts a fresh scratch Bus (default size 160x45) and waits until it answers
`bus state` and has drawn its first room. Prints the session status with
`ready_ms`. Refused when the session name is already running, when 4
sessions run, when the target belongs to a live Bus, or when inherited
`BUS_*`/`HERDR_*` variables point at it.

### stop
`stop [--name NAME]`

Quits the scratch Bus, stops its server, kills anything still holding the
session directory, copies the Bus logs into the run folder and removes the
session directory. Prints the run folder.

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
frame `seq`. `--plain` prints just the rows, for reading.

### find
`find TEXT [--regex] [--rows A-B]`

Every visible match, with its row, column and width in cells.

### cells
`cells ROW COL [WIDTH HEIGHT]`

Cells with their symbol, `fg`/`bg` colours (`#rrggbb`, `idx:N` or
`default`), attributes (bold, dim, italic, underline, inverse, …) and
hyperlink. Use it for colour questions.

### type
`type TEXT`

Types text as keystrokes.

### paste
`paste TEXT`

Pastes text, bracketed when the client enabled bracketed paste.

### press
`press KEY [KEY …]`

Presses keys in order: `enter`, `esc`, `tab`, `shift+tab`, `backspace`,
`up`, `down`, `left`, `right`, `f1`…`f12`, `space`, letters, and chords such
as `ctrl+n`, `ctrl+shift+r`, `alt+up`. Keys are encoded the way Bus's
terminal negotiated, so Esc never waits out a timeout.

### click
`click (ROW COL | --text TEXT [--regex] [--nth N] [--rows A-B]) [--double] [--button left|middle|right] [--mods shift,alt,ctrl]`

Clicks a cell, or the middle of a visible text match. A text that matches
more than once needs `--nth N` or `--rows`. `--double` sends two clicks
inside Bus's double-click window.

### drag
`drag ROW1 COL1 ROW2 COL2 | --from-text A --to-text B [--button B] [--mods M]`

Presses at the first cell, moves, releases at the second. When Bus copies
the selection, the copied text is in `copied` (Bus copies with OSC 52 in
driver sessions, so your clipboard is never touched).

### scroll
`scroll (ROW COL | --text TEXT) (--up N | --down N) [--mods M]`

Wheel notches at a cell.

### resize
`resize COLSxROWS`

Resizes the terminal; Bus relays out.

### wait
`wait --text TEXT [--regex] [--gone] [--rows A-B] [--timeout 10s]`
`wait --stable 300ms [--rows A-B] [--timeout 10s]`
`wait --state 'PATH OP VALUE' [--timeout 10s]`
`wait --message ID [--timeout 60s]`

Waits for text to appear (or with `--gone`, to disappear), for rows to stay
unchanged, for `bus state` to match, or for a message to settle. Exit 1 on
timeout, with a screen excerpt or the actual state value. Never sleep
instead. Scope `--stable` with `--rows`: agent spinners and toasts keep the
whole screen moving.

State predicates: PATH is dotted into the `bus state` result (`visible_room`,
`rooms.2.name`, `agents.*.status`; `*` means any element). OP is `==`, `!=`,
`contains`, `>`, `>=`, `<` or `<=`. VALUE is JSON, or a bare word taken as a
string, for example `agents.*.status == idle`.

### expect
`expect …`

The same as `wait`, read as an assertion.

### clipboard
`clipboard`

Every copy Bus made in this session (OSC 52).

### journey
`journey STEPS.json`

Runs steps in one session and stops at the first failure. The file is a JSON
list of `{"verb": "press", "args": ["ctrl+n"], "pause_ms": 0}`. `start`,
`stop`, `bus` and `journey` are not allowed as steps.

### gc
`gc [--older-than 7d] [--run-dir DIR]`

Deletes old run folders; folders of running sessions are kept.
