Dev tools (answer only in a session started with `bus --dev`):
  room focus ROOM
  room seen ROOM
  room sound ROOM (--on | --off) [--sound NAME]
  agent focus AGENT
  agent details AGENT (--on | --off)
  settings color-blind (--on | --off)
  settings room-sound (--on | --off) [--sound NAME]
  settings --error-orchestrator NAME|off
  sounds
  quit
  diagnostics
  ui hits      (dev variant only)
  restart --binary PATH --confirm
  dashboard

Bus's control commands come in two tiers. The agent tier (`bus --help`) is how
orchestrators and workers coordinate, and every session answers it. Dev tools
inspect or change Bus's own UI, the developer's view and preferences, and Bus's
internals. A session answers them only if it was started with `--dev`;
otherwise they fail with dev_tools_disabled. The `--dev` flag of the session a
command targets decides, not the flag on the command line, and every build
ships the dev tools, release and npm installs included. The gate keeps dev
tools out of normal sessions' agents and help text. It is not a security
boundary: any process of the same user can reach the session's private
control.sock.
Separately, test tooling (the TUI driver, `bus --dev tui`) is compiled only into the dev
variant: cargo feature `dev-tools`, built by `./run dev`. The release variant (`./run prod`,
`just build`, Nix, npm) leaves it out entirely, and its `tui` command only says so.
focus queues a visible Bus view change; its receipt does not claim the view has rendered.
room seen clears a room's unread count and unseen-to-You dot without changing
the visible Bus view.
room sound turns that room's new-message sound on or off; MASTER starts on, work rooms off.
room sound --sound picks a system sound by name (Default is Bus's own ding); sounds lists them.
room sound master changes MASTER's sound. settings room-sound sets the All rooms sound on
every work room at once and saves it for rooms created later. Bus launches and room
creation read them.
agent details and settings color-blind set the TUI toggles; state shows both.
quit queues the TUI's save-and-quit (as Ctrl+Q); its receipt only attests queuing.
The UI saves unsent drafts, then stops the session's server and every agent pane, as
bus stop does, and exits; bus resume relaunches each agent into its saved conversation.
ui hits lists the room UI's clickable regions as of its last frame, each with a stable
name (room:NAME, agent:NAME, new-room, settings, color-blind, composer, files, ...) and its
cells; the TUI driver clicks them by name. Only the dev variant (./run dev) has it.
restart (`bus --dev restart --binary PATH --confirm`) saves and quits the whole
Bus, then relaunches PATH in the same terminal with the recorded flags and session.
Both the server and TUI/coordinator must run PATH's build SHA before success is
recorded. An unhealthy replacement relaunches the saved previous binary instead.
Agents resume their saved provider sessions. Use the absolute binary path agents
already call (for example `target/debug/bus`); other CLI paths are not rewritten.
The initial `stage:handoff` receipt only means queuing; read `restarts/log.jsonl`
for the outcome and any manual recovery command. It is refused without `--dev` and
omitted from `bus --help`. Before it, every room's orchestrator asks working agents
to pause (commit and push, stop builds and tests, release slot locks, then stop)
and waits for each to confirm. After the outcome notice, tell them to resume.
See `bus --dev restart --help`.
diagnostics shows Bus's internals: version, storage pause, coordinator error, data and log
paths, and each agent's status, wait reason, detail, runtime identity and current request.

Error reporting defaults to Off. In Settings > Developer, Bus errors opens a
dropdown containing Off and current MASTER orchestrator names. The CLI option
above saves the same `error_orchestrator` setting; dev settings output includes
its value and `error_orchestrators` choices. A missing/deleting recipient is inactive.

The shared file-logging hook captures every ERROR and every WARN except delay-only
perf/load/stall/slow-operation/recovery events, `api.wait.timeout`, host-key escape
timeouts, expected form/MASTER-rename/MASTER-delete/image-attachment/control-capacity/
callback refusals, and normal existing-server/forced-quit/process-exit/kept-terminal/
expired-paste-recovery notices. The exact excluded event names live in
`utils::error_pipe::broken_warning`. Warnings such as failed persistence, clipboard,
API response, provider launch, callback capture and unavailable instrumentation count.

Reports contain only `Bus error: EVENT (xN since last notice). Logs: PATH`.
Event plus source callsite plus log path is the dedupe key. The first burst waits
one second; new repeats are counted and summarized at most once per minute.
Off clears pending counts and old inbox records. Report failures stay in the logs
without being captured again. No error toast, inline error, sidebar or footer text
is added; the short targeted message uses ordinary Bus conversation history.

`bus --dev dashboard` is a local file reader: it works with an ordinary or dev
session, including an exited server. It prints a random `http://127.0.0.1:PORT/`
URL, serves one page with inline assets, and refreshes every thirty seconds.
`BUS_DATA_DIR` selects an exact session root; otherwise the last session is read
without recording a resume. It reads persisted state, journal and archived
requests; it never sends control commands or saves session state. Stop with Ctrl+C.
The page shows local send-hour delivery and reply capture rates, denominators,
open requests, provider groups, and failure reasons. Bus reliability counts replied
versus Bus-caused terminal failures, excluding deliberate interrupts, deleted
agents/rooms, provider errors and pending requests. The full classification is
available in a collapsed counting reference and metric tooltips. Read/HTTP errors go only to `dashboard.log` in the session's
logs directory; unavailable data appears as “No data”.

Settings > Developer > Open delivery dashboard opens the attached session's
page in the default browser. Repeated clicks and the dev CLI reuse that session's
service for the current build. Discovery verifies the serving build; leases live
under `dashboard-runtime/BUILD_SHA/`, and a new build cooperatively retires the
previous service. These runtime files are separate from message/request records.
The whole-session headline is followed by an hourly graph with attempt counts
and a selectable recipient breakdown; provider and room grouping are available.
Every failure count or cause opens request metadata and a copyable
`bus message status MESSAGE_ID` command, with causes ordered by count.

The Agent launches view shows usable / (usable + failed) for tracked creation,
resume and recovery attempts, overall and by local start hour and provider.
Pending attempts and deliberate cancellations are shown separately. Usability
requires identity-matched process and prompt readiness, or a trusted new-message
start; saved identity and failed native observations do not establish success or
failure. Native restores use their first-observed time. The view labels when
tracking began: earlier launches have no durable outcome history and are not
backfilled from the current roster. Failure counts and causes open a bounded list
with the saved attempt identity, provider, time, cause and log/record references.
`bus --paths` locates that session's log and state directories.
