//! CLI help text kept separate from parsing and transport.
use super::session_pick::USAGE;

pub const HELP: &str = "Developer commands (require an already running Bus --dev instance):
  state
  room create NAME
  room rename ROOM NAME
  room notes ROOM --text TEXT
  room delete ROOM --confirm
  room focus ROOM
  room seen ROOM
  room sound ROOM (--on | --off) [--sound NAME]
  agent add --room ROOM --name NAME --provider claude|codex|cursor --pwd PATH
            [--args STRING] [--consent-hooks] [--orchestrates ROOM]
            [--system-prompt TEXT | --system-prompt-file PATH]
  agent read AGENT --source visible
  agent read AGENT [--source recent] --lines N
  agent dialog AGENT
  agent choose AGENT --option N --fingerprint FINGERPRINT
  agent answer AGENT (--text TEXT | --skip) --fingerprint FINGERPRINT
  agent focus AGENT
  agent clear AGENT
  agent rename AGENT NAME
  agent details AGENT (--on | --off)
  agent setup-confirm AGENT --confirm
  agent delete AGENT --confirm
  send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...] [--as AGENT] [--queue] [--async]
  message status MESSAGE_ID
  request recover REQUEST_ID --confirm
  wait --message MESSAGE_ID [--timeout SECONDS]
  history --room ROOM
  settings
  settings color-blind (--on | --off)
  settings room-sound (--on | --off) [--sound NAME]
  quit
  diagnostics
  sounds

Every command accepts --request-id STRING and emits one JSON response.
ROOM and AGENT accept a name or numeric ID; ROOM also accepts master (any case) for the
MASTER room. Every MASTER agent orchestrates exactly one work room for its whole life:
agent add --room master --orchestrates ROOM is required, a work room has at most one
orchestrator, and deleting the room also deletes its orchestrator.
A MASTER agent launches with an orchestrator system prompt, the built-in one unless
--system-prompt or --system-prompt-file replaces it; {{ROOM_NAME}} {{ROOM_ID}} {{AGENT_NAME}}
{{DOCS}} are filled in. Reassigning sends the orchestrator a message naming its new room.
--args \"--resume SESSION_ID\" (claude, cursor) or \"resume SESSION_ID\" (codex) adopts an
existing provider session by its UUID; quit that session elsewhere first.
Use --to all explicitly for all room agents.
agent dialog shows a choice dialog or focused free-text question and a single-use
fingerprint. agent choose selects an option with Up/Down and Enter; agent answer pastes
text and presses Enter, or skips the question. Both require the exact observed dialog.
Bus messages a room's orchestrator about each dialog, and
wait stops early with agent_waiting_on_dialog when a recipient shows one.
send --as records the message as written by that room agent or the room's MASTER
orchestrator; --to all then skips it.
send --room master --as AGENT --to human posts a MASTER agent's message to the human
in MASTER chat, with no agent recipient and nothing to wait for; it rings like a reply.
agent clear starts a fresh provider context in an idle agent's terminal (/clear for claude
and codex, /new-chat for cursor) and keeps the agent bound to the new provider session.
room seen clears a room's unread count without changing the visible Bus view.
room sound turns that room's new-message sound on or off; MASTER starts on, work rooms off.
room sound --sound picks a system sound by name (Default is Bus's own ding); sounds lists them.
settings shows the settings every Bus shares: color blind mode, MASTER's sound (room sound
master changes it) and room_sound, the All rooms sound: settings room-sound sets it on every
work room at once and saves it for rooms created later.
Bus launches and room creation read them; state shows each room's effective sound.
state includes each agent's compactions and per-provider usage (5-hour and weekly used %).
Claude usage comes from its status line; Codex usage is read after each turn.
Usage status \"unknown\" means data is missing or stale, never that the allowance is unused.
wait polls every 200 ms, defaults to 60 seconds, and accepts 1–600 seconds.
send --async prints the send receipt, then blocks with no time limit until every recipient
has worked on the message and gone idle again (a blocked recipient keeps it waiting), and
prints the final message status. It exits 1 if the send fails or Bus abandons a
recipient's request, and 3 if a message stalls (stage stalled, reason in message status).
wait also exits 3 on a stall. A Blocked agent never stalls; after 5 minutes its reason
reads blocked_unanswered. Orchestrators run it as a background tool call and read the reply
with history when it exits.
message status, wait and history keep raw Markdown and list attached files as absolute paths.
focus queues a visible Bus view change; its receipt does not claim the view has rendered.
agent read also works while an agent is launching (e.g. to see a provider trust prompt);
runtime.session_verified is false until its provider session starts.
agent details and settings color-blind set the TUI toggles; state shows both.
quit queues the TUI's save-and-quit (as Ctrl+Q); its receipt only attests queuing.
quit leaves the session server and its agents running for bus resume; to end them, run
bus stop (no --dev needed), which stops the server, closes every agent pane and prints
{\"stopped\":true}, or {\"stopped\":false} when no server was running.
Commands only connect to the existing instance in BUS_DATA_DIR; they never start or enable it.";

pub(super) fn print_help() {
    println!("{}\n", HELP);
    println!("Bus — coordinate selected agents in native terminal rooms\n\n{USAGE}\n\nA plain `bus` launch always creates a new local session.\n`bus sessions` lists resumable sessions, their rooms, and recent activity.\n`bus resume <session-id>` resumes that exact session.\n`bus resume --last` resumes the last opened session.\n`bus stop` stops the session's server and closes its agent panes; quit an open UI first.\n--dev enables developer log files, excluding input/content dumps.\nExisting servers keep their original log level; they are never automatically restarted.\n--paths shows data and log directories without starting a session.\nBUS_DATA_DIR is an exact isolated-root override for development and tests; it cannot be combined with resume.\n\nCtrl+Shift+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+A/E line start/end · Ctrl+R history search · Ctrl+Shift+E composer size\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved.");
}

/// Reopens this Bus session: a local session by its ID, an explicit
/// `BUS_DATA_DIR` root by launching Bus again with the same environment.
pub fn local_attach_command() -> String {
    attach_command_for(std::env::var("BUS_SESSION_ID").ok().as_deref())
}

pub(crate) fn attach_command_for(session_id: Option<&str>) -> String {
    match session_id {
        Some(id) if !id.is_empty() => format!("bus resume {id}"),
        _ => "bus".to_string(),
    }
}

/// `bus stop` targets `BUS_DATA_DIR`, else the last opened local session,
/// which opening this session has just recorded.
pub fn local_stop_command() -> String {
    "bus stop".to_string()
}

pub fn restart_after_update_guidance(stop_command: &str, attach_command: &str) -> String {
    format!(
        "Stop the old server to use the new version.\nStopping exits pane processes.\nRun `{stop_command}`, then run `{attach_command}` again."
    )
}

pub fn active_restart_after_update_guidance() -> String {
    restart_after_update_guidance(&local_stop_command(), &local_attach_command())
}
