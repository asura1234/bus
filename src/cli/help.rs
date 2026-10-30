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
pub const AGENT_NOTES: &str =
    "Control commands emit one JSON envelope, including failures; require ok:true.
Use bus <command> --help for behavior, defaults, result fields, errors and examples.
Control commands target an existing session: BUS_DATA_DIR, else the last opened one.
Every MASTER agent orchestrates exactly one work room for its whole life.
Read the short how-to-bus-cli.md for the room/message/turn model and workflows.";

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
    write_stdout_line(format_args!("Bus — coordinate selected agents in native terminal rooms\n\n{USAGE}\n\nA plain `bus` launch always creates a new local session.\n`bus sessions` lists resumable sessions, their rooms, and recent activity.\n`bus resume <session-id>` resumes that exact session.\n`bus resume --last` resumes the last opened session.\n`bus stop` stops the session's server and closes its agent panes; quit an open UI first.\n`bus restart --binary PATH --confirm` restarts this session onto PATH. Pause agents first; see how-to-bus-cli.md.\n`bus perf` summarizes the session's perf records (stalls, slow windows, CPU/memory/load peaks;\ndefault last 30 min) from perf/ in the logs directory (see --paths); it reads files only, so it works while Bus is hung.\n--dev starts a session with dev tools on (listed by `bus --dev --help`) and developer log files,\nexcluding input/content dumps.\nExisting servers keep their original log level; they are never automatically restarted.\n--paths shows data and log directories without starting a session.\nBUS_DATA_DIR is an exact isolated-root override for development and tests; it cannot be combined with resume.\n\nCtrl+Shift+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+A/E line start/end · Ctrl+R history search · Ctrl+Shift+E composer size\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved."));
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
