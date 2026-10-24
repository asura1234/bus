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

/// General command behavior; command and flag inventories come from the grammar.
pub const AGENT_NOTES: &str = "Every command accepts --request-id STRING and emits one JSON response.
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
It returns a single-use fingerprint. agent choose selects a native row with Up/Down and Enter,
including Claude checkbox toggles and Submit/Next; its question-tab options send Left/Right.
Use the returned option numbers. agent answer pastes
text and presses Enter, or skips the question (never Claude's: Esc cancels its form); on a
choice with a free_text option it
moves to that row, types the text and submits it. Both require the exact observed dialog.
On Claude multi-select fields, pasting ticks the custom answer; Bus leaves it checked
without Enter, then choose Next or Submit. For every provider, including Codex Other
and Cursor fields, existing drafts refuse further text (free_text_holds_draft);
edit or submit the draft in the agent's pane.
Bus messages a room's orchestrator about each dialog, and
wait stops early with agent_waiting_on_dialog when a recipient shows one.
send --as records the message as written by an agent; --to all then skips it.
Current agent names are unique across all rooms, including MASTER; add/rename reject duplicates.
Every AGENT selector accepts a name or numeric ID. agent list prints name -> id -> room rows.
Agent commands with an AGENT selector accept optional --room ROOM and --as AGENT for conversation context.
Any agent can message any other agent across rooms. Names and exact IDs resolve globally;
an existing numeric ID wins over a numeric name. --room chooses the conversation, not a name scope.
Only when no live candidate matches, removed recipient IDs/names forward to a unique same-name
replacement in the original room, or that room's current orchestrator. Removed orchestrators
forward through the work room they orchestrated. Authors and administrative commands never forward.
The case-insensitive --to orchestrator alias names the sender's home-room orchestrator.
All still covers conversation-room members only. Cross-room messages wait for a new turn.
Bus adds one sender header: 'name:' locally, 'name (room R):' across rooms or MASTER.
IDs appear in headers only for numeric or historical names; batch markers carry position and time.
Send only the body: do not add a 'from X', 'X here' or signature line.
An exact genuine current/legacy header or an identity-only name/agent ID line is normalized to this header.
Ordinary sentences and headers with a forged ID or room always get the real sender header.
send --room ROOM --as AGENT --to you posts any agent's message to the developer in every room.
It stays in that room, addressed to You, with no agent request or wait. --to human is an alias.
Messages to you and every [waiting-on-you] always notify with a distinct sound and Feishu bot DM;
room sounds and legacy Feishu off settings cannot silence them. Developer sends never notify.
Use @you or [waiting-on-you] for things only the developer can do or decide.
agent interrupt stops an agent's running turn: Bus presses the provider's stop key once
(Esc for claude and codex, Ctrl+C for cursor), waits up to 15 s until the agent is idle,
and reports stopped, already_idle, or failed with a reason (exit 1). The turn, the messages
typed into it and those queued for the agent close as interrupted, so wait and send --async
end with message_interrupted; the room history records who interrupted whom. --text then
sends a normal new message; --room stops every working or blocked agent in the room but the --as one.
It kills no process and keeps the context; use send to steer a working agent instead.
agent clear starts a fresh provider context in an idle agent's terminal (/clear for claude
and codex, /new-chat for cursor) and keeps the agent bound to the new provider session.
settings shows the settings every Bus shares: color blind mode, MASTER's sound and
room_sound, the All rooms sound. state shows each room's effective sound.
state includes each agent's compactions, its wait_reason (why a queued message would wait on
it, or null) and per-provider usage (5-hour and weekly used %).
Claude usage comes from its status line; Codex usage is read after each turn.
Usage status \"unknown\" means data is missing or stale, never that the allowance is unused.
Bounded wait polls every 200 ms, defaults to 60 seconds, and accepts 1–600 seconds.
Async waits have no deadline and back off from 200 ms to 2 s; wait --async stops on settlement or a dialog.
Both async waits exit 1 with bus_down after four consecutive transport failures.
Live-server overload replies and accepted-connection disconnects back off and retry.
send --async blocks with no time limit until the message is complete: every recipient
replied or Bus closed its request (a blocked recipient keeps it waiting). It prints one JSON object
with the final message status, message_id and send_receipt. It exits 1 if Bus is unreachable,
the send fails or Bus abandons a recipient's request, and 3 if a message stalls
(stage stalled, reason in message status).
wait also exits 3 on a stall. A Blocked agent never stalls; after 5 minutes its reason
reads blocked_unanswered. Orchestrators run it as a background tool call and read the reply
with history when it exits.
message status, wait and history keep raw Markdown and list attached files as absolute paths.
Worker replies to their room orchestrator also wake its terminal. Author waiters
(send --async --as, or wait --as) defer that wake while alive and mark flushed replies
seen; message status and anonymous waits never suppress delivery. message seen is
the post-output acknowledgement; do not use it before reading the reply.
agent read also works while an agent is launching (e.g. to see a provider trust prompt);
runtime.session_verified is false until its provider session starts.
While an agent on the alternate screen (Codex) is busy, a recent read returns its visible screen
with capture.exhausted false and capture.note saying why; retry when it is idle for older lines.
Every control command emits exactly one JSON object, including failures.
Every session answers these commands.";

/// Dev-tier commands and notes, printed after the agent tier by
/// `bus --dev --help` only. The repo doc is the help source, so the two
/// cannot drift; it is never written to the agents' docs folder.
pub const DEV_HELP: &str = include_str!("../../docs/dev-tools.md");

pub(crate) fn agent_help() -> String {
    format!("{}\n{AGENT_NOTES}", super::control::overview(false))
}

pub(super) fn subcommand_help(args: &[String]) -> Option<String> {
    if args
        .iter()
        .all(|arg| matches!(arg.as_str(), "--help" | "-h" | "--dev" | "--yolo"))
    {
        return None;
    }
    let dev = args
        .iter()
        .take_while(|arg| arg.starts_with('-'))
        .any(|arg| arg == "--dev");
    super::control::help_for(super::control::command_tree(dev), args)
}

pub(super) fn print_help(dev: bool) {
    write_stdout_line(format_args!(
        "{}",
        super::control::command_tree(dev).render_long_help()
    ));
    let overview = if dev {
        super::control::overview(true)
    } else {
        agent_help()
    };
    write_stdout_line(format_args!("{overview}\n"));
    if dev {
        write_stdout_line(format_args!("{AGENT_NOTES}\n"));
    }
    write_stdout_line(format_args!("--yolo bypasses provider permission prompts for all workers and MASTER orchestrators\n(new and resumed) in this run only. It is off by default and is never saved.\n"));
    if dev {
        write_stdout_line(format_args!("{}", DEV_HELP));
    }
    write_stdout_line(format_args!("Bus — coordinate selected agents in native terminal rooms\n\n{USAGE}\n\nA plain `bus` launch always creates a new local session.\n`bus sessions` lists resumable sessions, their rooms, and recent activity.\n`bus resume <session-id>` resumes that exact session.\n`bus resume --last` resumes the last opened session.\n`bus stop` stops the session's server and closes its agent panes; quit an open UI first.\n`bus perf` summarizes the session's perf records (stalls, slow windows, CPU/memory/load peaks;\ndefault last 30 min) from perf/ in the logs directory (see --paths); it reads files only, so it works while Bus is hung.\n--dev starts a session with dev tools on (listed by `bus --dev --help`) and developer log files,\nexcluding input/content dumps.\nExisting servers keep their original log level; they are never automatically restarted.\n--paths shows data and log directories without starting a session.\nBUS_DATA_DIR is an exact isolated-root override for development and tests; it cannot be combined with resume.\n\nCtrl+Shift+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+A/E line start/end · Ctrl+R history search · Ctrl+Shift+E composer size\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved."));
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
