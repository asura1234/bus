Dev tools (answer only in a session started with `bus --dev`):
  room focus ROOM
  room seen ROOM
  room sound ROOM (--on | --off) [--sound NAME]
  agent focus AGENT
  agent details AGENT (--on | --off)
  settings color-blind (--on | --off)
  settings room-sound (--on | --off) [--sound NAME]
  sounds
  quit
  diagnostics

Bus's control commands come in two tiers. The agent tier (`bus --help`) is how
orchestrators and workers coordinate, and every session answers it. Dev tools
inspect or change Bus's own UI, the human's view and preferences, and Bus's
internals. A session answers them only if it was started with `--dev`;
otherwise they fail with dev_tools_disabled. The `--dev` flag of the session a
command targets decides, not the flag on the command line, and every build
ships the dev tools, release and npm installs included. The gate keeps dev
tools out of normal sessions' agents and help text. It is not a security
boundary: any process of the same user can reach the session's private
control.sock.
focus queues a visible Bus view change; its receipt does not claim the view has rendered.
room seen clears a room's unread count without changing the visible Bus view.
room sound turns that room's new-message sound on or off; MASTER starts on, work rooms off.
room sound --sound picks a system sound by name (Default is Bus's own ding); sounds lists them.
room sound master changes MASTER's sound. settings room-sound sets the All rooms sound on
every work room at once and saves it for rooms created later. Bus launches and room
creation read them.
agent details and settings color-blind set the TUI toggles; state shows both.
quit queues the TUI's save-and-quit (as Ctrl+Q); its receipt only attests queuing.
The UI saves unsent drafts, then stops the session's server and every agent pane, as
bus stop does, and exits; bus resume relaunches each agent into its saved conversation.
diagnostics shows Bus's internals: version, storage pause, coordinator error, data and log
paths, and each agent's status, wait reason, detail, runtime identity and current request.
