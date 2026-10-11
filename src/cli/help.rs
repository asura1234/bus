//! CLI help text kept separate from parsing and transport.
use super::session_pick::USAGE;
pub(crate) use crate::client::errors::local_attach_command;
use std::io::{self, Write};

/// CLI and observation-hook replies use the same locked stdout writer as the
/// control CLI. Keep println!'s newline and panic-on-write-error behavior;
/// these bytes are command/protocol output, not diagnostic log messages.
pub(crate) fn write_stdout_line(arguments: std::fmt::Arguments<'_>) {
    if let Err(error) = writeln!(io::stdout().lock(), "{arguments}") {
        panic!("failed printing to stdout: {error}");
    }
}

/// Agent-tier commands, which every session answers. `bus --help` prints only
/// these; the tier of each command is `coordinator::control::METHODS`.
pub const AGENT_HELP: &str = "Commands (connect to the running Bus in BUS_DATA_DIR; they never start one):
  state
  room create NAME
  room rename ROOM NAME
  room notes ROOM --text TEXT
  room delete ROOM --confirm
  agent add --room ROOM --name NAME --provider claude|codex|cursor --pwd PATH
            [--args STRING] [--consent-hooks] [--orchestrates ROOM]
            [--system-prompt TEXT | --system-prompt-file PATH]
  agent read AGENT --source visible
  agent read AGENT [--source recent] --lines N
  agent dialog AGENT
  agent choose AGENT --option N --fingerprint FINGERPRINT
  agent answer AGENT (--text TEXT | --skip) --fingerprint FINGERPRINT
  agent clear AGENT
  agent rename AGENT NAME
  agent setup-confirm AGENT --confirm
  agent delete AGENT --confirm
  send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...] [--as AGENT] [--queue] [--async]
  message status MESSAGE_ID
  request recover REQUEST_ID --confirm
  wait --message MESSAGE_ID [--timeout SECONDS]
  history --room ROOM
  settings

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
agent dialog opens queued Codex questions and shows choices or a focused text question.
It returns a single-use fingerprint. agent choose selects an option with Up/Down and Enter; agent answer pastes
text and presses Enter, or skips the question. Both require the exact observed dialog.
Bus messages a room's orchestrator about each dialog, and
wait stops early with agent_waiting_on_dialog when a recipient shows one.
send --as records the message as written by that room agent or the room's MASTER
orchestrator; --to all then skips it.
send --room master --as AGENT --to human posts a MASTER agent's message to the human
in MASTER chat, with no agent recipient and nothing to wait for; it rings like a reply.
agent clear starts a fresh provider context in an idle agent's terminal (/clear for claude
and codex, /new-chat for cursor) and keeps the agent bound to the new provider session.
settings shows the settings every Bus shares: color blind mode, MASTER's sound and
room_sound, the All rooms sound. state shows each room's effective sound.
state includes each agent's compactions, its wait_reason (why a queued message would wait on
it, or null) and per-provider usage (5-hour and weekly used %).
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
agent read also works while an agent is launching (e.g. to see a provider trust prompt);
runtime.session_verified is false until its provider session starts.
Every session answers these commands.";

/// Dev-tier commands and notes, printed after the agent tier by
/// `bus --dev --help` only. The repo doc is the help source, so the two
/// cannot drift; it is never written to the agents' docs folder.
pub const DEV_HELP: &str = include_str!("../../docs/dev-tools.md");

pub(super) fn print_help(dev: bool) {
    write_stdout_line(format_args!("{}\n", AGENT_HELP));
    if dev {
        write_stdout_line(format_args!("{}", DEV_HELP));
    }
    write_stdout_line(format_args!("Bus — coordinate selected agents in native terminal rooms\n\n{USAGE}\n\nA plain `bus` launch always creates a new local session.\n`bus sessions` lists resumable sessions, their rooms, and recent activity.\n`bus resume <session-id>` resumes that exact session.\n`bus resume --last` resumes the last opened session.\n`bus stop` stops the session's server and closes its agent panes; quit an open UI first.\n--dev starts a session with dev tools on (listed by `bus --dev --help`) and developer log files,\nexcluding input/content dumps.\nExisting servers keep their original log level; they are never automatically restarted.\n--paths shows data and log directories without starting a session.\nBUS_DATA_DIR is an exact isolated-root override for development and tests; it cannot be combined with resume.\n\nCtrl+Shift+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+A/E line start/end · Ctrl+R history search · Ctrl+Shift+E composer size\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved."));
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

#[cfg(test)]
mod tests {
    use super::restart_after_update_guidance;
    use crate::client::errors::attach_command_for;

    include!("tests/session_guidance_test.rs");
}
