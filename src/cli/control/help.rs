//! Help metadata for the control grammar. Command and flag inventories come from Clap.
use crate::messaging::coordinator::{Tier, METHODS};
use clap::{Arg, Command};
#[path = "help_details.rs"]
mod details;

pub(super) fn decorate(command: Command, path: &str, dev: bool) -> Command {
    let hidden = path == "ui"
        || METHODS
            .iter()
            .any(|spec| spec.command == path && spec.tier == Tier::Dev);
    let (about, example) = description(path);
    let command = command
        .hide(hidden && !dev)
        .about(about)
        .long_about(format!(
            "{about}\n\n{}\n\n{}",
            details::command(path),
            details::CONTROL
        ))
        .after_help(example);
    let args: Vec<_> = command
        .get_arguments()
        .map(|arg| arg.get_id().as_str().to_owned())
        .collect();
    let command = args.into_iter().fold(command, |cmd, id| {
        cmd.mut_arg(&id, |arg| match details::default_value(path, &id, &arg) {
            Some(default) => arg.help(format!("{} [{default}]", argument_help(&id))),
            None => arg.help(argument_help(&id)),
        })
    });
    command.mut_subcommands(|child| {
        let child_path = if path.is_empty() {
            child.get_name().to_owned()
        } else {
            format!("{path} {}", child.get_name())
        };
        decorate(child, &child_path, dev)
    })
}

fn argument_help(id: &str) -> &'static str {
    match id {
        "request-id" => "Use this request identity for retries and the JSON response",
        "room" => "Room name or ID (master selects MASTER)",
        "agent" => "Agent name or ID",
        "name" => "Name to create or assign",
        "provider" => "Provider used to launch the agent",
        "pwd" => "Project working directory for the agent",
        "args" => "Additional provider launch arguments as one string",
        "consent-hooks" => "Approve installing or updating provider hooks in the project",
        "orchestrates" => "Work room this MASTER agent will orchestrate",
        "system-prompt" => "Replace the orchestrator system prompt with this text",
        "system-prompt-file" => "Read the replacement orchestrator system prompt from a file",
        "source" => "Read the visible viewport or recent terminal history",
        "lines" => "Positive line count required for a recent terminal read",
        "limit" => "Read at most this many newest history messages (positive integer)",
        "before" => "Read messages older than this message ID; requires --limit",
        "option" => "Option number returned by agent dialog",
        "fingerprint" => "Single-use fingerprint returned by agent dialog",
        "text" => "Message, answer, notes, or follow-up text as one argument",
        "skip" => "Skip the observed question instead of entering text",
        "as" => "Author agent name or ID for attribution and conversation context",
        "to" => "Comma-separated agent selectors, all, orchestrator, or you (human)",
        "file" => "Attach a file by path; repeat for multiple files",
        "queue" => "Queue delivery until the selected agents are idle",
        "async" => "Wait without an overall deadline for agent replies or settlement",
        "message" => "Message ID returned by send",
        "reply-to" => {
            "Answer this message and complete only your recipient request, without ending your turn. Excludes --room, --to, --queue, --async and --file. Run in the foreground and check ok:true and stage:\"replied\" before claiming delivery; if it fails, check the existing answer with bus message status N before resending"
        }
        "request" => "Request ID returned by message status",
        "timeout" => "Wait deadline in seconds, from 1 through 600",
        "confirm" => "Explicitly approve this setup, recovery, or destructive operation",
        "on" => "Enable the setting",
        "off" => "Disable the setting (Feishu developer notifications are always on)",
        "sound" => "Sound name (use sounds to list installed choices)",
        _ => "Command argument",
    }
}

fn description(path: &str) -> (&'static str, &'static str) {
    match path {
        "" => ("Coordinate coding agents in terminal rooms", "Example: bus agent list"),
        "state" => ("Inspect rooms, agents, and current activity", "Example: bus state"),
        "quit" => ("Quit the running Bus", "Example: bus --dev quit"),
        "settings" => ("Inspect shared settings or choose a setting to update", "Example: bus settings"),
        "settings feishu-notifications" => ("Inspect or enable mandatory developer notifications", "Example: bus settings feishu-notifications --on"),
        "settings color-blind" => ("Set color blind mode", "Example: bus --dev settings color-blind --on"),
        "settings room-sound" => ("Set the All rooms sound", "Example: bus --dev settings room-sound --on --sound Glass"),
        "diagnostics" => ("Inspect internal session diagnostics", "Example: bus --dev diagnostics"),
        "sounds" => ("List installed notification sounds", "Example: bus --dev sounds"),
        "ui" => ("Inspect the developer UI", "Example: bus --dev ui hits"),
        "ui hits" => ("Read TUI hit regions", "Example: bus --dev ui hits"),
        "room" => ("Create and manage rooms", "Example: bus room create feature-work"),
        "room create" => ("Create a work room", "Example: bus room create feature-work\n         bus room create release-check --request-id release-check-room-v1"),
        "room focus" => ("Focus a room in the UI", "Example: bus --dev room focus feature-work"),
        "room seen" => ("Mark a room as seen", "Example: bus --dev room seen feature-work"),
        "room sound" => ("Set a room notification sound", "Example: bus --dev room sound feature-work --on --sound Glass"),
        "room rename" => ("Rename a work room", "Example: bus room rename feature-work implementation"),
        "room notes" => ("Replace a room's notes", "Example: bus room notes feature-work --text 'Review pending'"),
        "room delete" => ("Delete a room and its orchestrator", "Example: bus room delete feature-work --confirm"),
        "agent" => ("Launch, inspect, and manage agents", "Example: bus agent list"),
        "agent add" => ("Launch a named agent in a room", "Example: bus agent add --room feature-work --name worker --provider codex --pwd /path/to/project"),
        "agent list" => ("List current agent names, IDs, and rooms", "Example: bus agent list --room feature-work"),
        "agent read" => ("Read an agent's terminal", "Example: bus agent read worker --source recent --lines 80\n         bus agent read worker --source visible"),
        "agent dialog" => ("Observe an agent's active dialog and fingerprint", "Example: bus agent dialog worker"),
        "agent choose" => ("Select an option in the observed dialog", "Example: bus agent choose worker --option 1 --fingerprint FINGERPRINT"),
        "agent answer" => ("Answer or skip the observed question", "Example: bus agent answer worker --text 'Proceed' --fingerprint FINGERPRINT"),
        "agent focus" => ("Focus an agent's terminal", "Example: bus --dev agent focus worker"),
        "agent clear" => ("Start a fresh provider context in an idle agent", "Example: bus agent clear worker"),
        "agent resume" => ("Resume suspended agents in a room", "Example: bus agent resume --room feature-work"),
        "agent interrupt" => ("Stop a running turn and optionally send a follow-up", "Example: bus agent interrupt worker --as coordinator"),
        "agent details" => ("Set agent terminal details visibility", "Example: bus --dev agent details worker --on"),
        "agent rename" => ("Rename an agent", "Example: bus agent rename worker implementer"),
        "agent setup-confirm" => ("Record project hook consent for an agent", "Example: bus agent setup-confirm worker --confirm"),
        "agent delete" => ("Delete an agent and close its terminal", "Example: bus agent delete worker --confirm"),
        "send" => ("Send a message or complete a request with a correlated reply", "Example: bus send --room feature-work --to worker --text 'Inspect the change'\n         bus send --reply-to 123 --as worker --text 'Inspection complete'"),
        "message" => ("Inspect delivery or acknowledge a reply", "Example: bus message status 123"),
        "message status" => ("Read message delivery status and replies", "Example: bus message status 123"),
        "message seen" => ("Acknowledge a reply after reading its output", "Example: bus message seen 456 --as coordinator"),
        "request" => ("Close a stuck request without replaying it", "Example: bus request recover 456 --confirm"),
        "request recover" => ("Close a stuck request without replaying it", "Example: bus request recover 456 --confirm"),
        "wait" => ("Wait for a message to finish or require attention", "Example: bus wait --message 123 --timeout 60\n         bus wait --message 123 --async"),
        "history" => ("Read room messages and delivery history", "Example: bus history --room feature-work\n         bus history --room feature-work --limit 100"),
        _ => ("Bus command", "Example: bus --help"),
    }
}

fn run_flags() -> [Arg; 2] {
    [
        Arg::new("dev")
            .long("dev")
            .action(clap::ArgAction::SetTrue)
            .help("Enable developer tools and developer logs [default: false]"),
        Arg::new("yolo")
            .long("yolo")
            .action(clap::ArgAction::SetTrue)
            .help("Bypass provider permission prompts for this run [default: false]"),
    ]
}

pub(crate) fn command_tree(dev: bool) -> Command {
    let control = super::parse::cli();
    let request_id = control
        .get_arguments()
        .find(|arg| arg.get_id() == "request-id")
        .expect("control request identity")
        .clone();
    let command = Command::new("bus")
        .bin_name("bus")
        .no_binary_name(true)
        .disable_help_subcommand(true)
        .subcommands(
            control
                .get_subcommands()
                .cloned()
                .map(|child| child.arg(request_id.clone())),
        );
    decorate(command, "", dev)
        .subcommand_required(false)
        .args(run_flags())
        .arg(
            Arg::new("paths")
                .long("paths")
                .action(clap::ArgAction::SetTrue)
                .help("Print data and log paths without starting Bus [default: false]"),
        )
        .subcommand(
            Command::new("skills")
                .about("Install or inspect default workflow skills at user scope")
                .long_about("Install or inspect Bus workflow skills in the current user's Claude and Codex skill directories without contacting a running Bus session.")
                .after_help("Example: bus skills status")
                .subcommand(
                    Command::new("status")
                        .about("Inspect skill copies without a session")
                        .long_about("Report the installed, outdated, and conflicting user skill copies as JSON without contacting a running Bus session.")
                        .after_help("Example: bus skills status"),
                )
                .subcommand(
                    Command::new("install")
                        .about("Copy default skills; preserve user skills")
                        .long_about("Copy Bus's default workflow skills into the current user's Claude and Codex directories while preserving modified or unrelated user skills. Use --yes to install without a prompt. The report's action for each skill is installed, updated, unchanged, preserved-conflict, or blocked-link.")
                        .after_help("Example: bus skills install --yes")
                        .arg(
                            Arg::new("yes")
                                .long("yes")
                                .action(clap::ArgAction::SetTrue)
                                .help("Install without prompting"),
                        ),
                ),
        )
        .subcommand(
            Command::new("sessions")
                .about("List saved sessions")
                .long_about(details::SESSIONS)
                .args(run_flags())
                .after_help("Example: bus sessions"),
        )
        .subcommand(
            Command::new("resume")
                .about("Resume a saved session")
                .long_about(details::RESUME)
                .args(run_flags())
                .group(
                    clap::ArgGroup::new("target")
                        .args(["session-id", "last"])
                        .required(true),
                )
                .arg(Arg::new("session-id").help("Saved session ID [required unless --last]"))
                .arg(
                    Arg::new("last")
                        .long("last")
                        .action(clap::ArgAction::SetTrue)
                        .help("Resume the last opened session [default: false]"),
                )
                .after_help("Example: bus resume --last"),
        )
        .subcommand(
            Command::new("stop")
                .about("Stop the session server and close its panes")
                .long_about(details::STOP)
                .args(run_flags())
                .after_help("Example: bus stop"),
        )
        .subcommand(
            Command::new("perf")
                .about("Summarize recorded performance; reads files only")
                .long_about(details::PERF)
                .arg(
                    Arg::new("since")
                        .long("since")
                        .help("Look back this duration, e.g. 30m or 2h; default 30m"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .conflicts_with("since")
                        .action(clap::ArgAction::SetTrue)
                        .help("Read all recorded performance history [default: false]"),
                )
                .after_help("Example: bus perf --since 2h"),
        )
}

pub(crate) fn help_for(command: Command, args: &[String]) -> Option<String> {
    let error = command.try_get_matches_from(args).err()?;
    matches!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelp
            | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    )
    .then(|| error.to_string())
}

pub(crate) fn overview(dev: bool) -> String {
    fn collect(command: &Command, path: &str, output: &mut String) {
        for child in command
            .get_subcommands()
            .filter(|child| !child.is_hide_set())
        {
            let path = if path.is_empty() {
                child.get_name().to_owned()
            } else {
                format!("{path} {}", child.get_name())
            };
            output.push_str(&format!(
                "  {path} — {}\n",
                child.get_about().map_or(String::new(), ToString::to_string)
            ));
            collect(child, &path, output);
        }
    }
    let mut output = String::from("Commands (use bus <command> --help for usage and flags):\n");
    collect(&command_tree(dev), "", &mut output);
    output
}
