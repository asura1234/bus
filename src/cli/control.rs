//! Client commands for an already running, explicitly enabled Bus developer instance.

use std::{
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use clap::{builder::NonEmptyStringValueParser, Arg, ArgAction, ArgMatches, Command};
use serde_json::{json, Value};

use crate::messaging::control::{self, Request, Response};

#[cfg(test)]
#[path = "tests/control_focus.rs"]
mod focus_tests;

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

/// The exit code of `wait` and `send --async` when a message stalled, so a
/// caller can tell "stuck, look at it" from a failed command (1).
pub(crate) const STALLED_EXIT_CODE: i32 = 3;

/// A failure that ends the process with its own code and a one-line reason on
/// stderr, instead of the generic error report.
#[derive(Debug)]
struct CliExit {
    code: i32,
    message: String,
}

impl std::fmt::Display for CliExit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliExit {}

pub fn run(data_dir: &Path, args: &[String]) -> io::Result<()> {
    let result = run_with(
        args,
        &mut io::stdout().lock(),
        |request, timeout| match timeout {
            Some(timeout) => control::request_with_timeout(data_dir, request, timeout),
            None => control::request(data_dir, request),
        },
    );
    if let Err(error) = &result {
        if let Some(exit) = error.get_ref().and_then(|e| e.downcast_ref::<CliExit>()) {
            eprintln!("bus: {exit}");
            std::process::exit(exit.code);
        }
    }
    result
}

fn next_request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "bus-dev-{}-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
    )
}

fn run_with(
    args: &[String],
    output: &mut impl Write,
    mut send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
) -> io::Result<()> {
    run_with_pause(args, output, &mut send, thread::sleep)
}

fn write_response(output: &mut impl Write, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *output, response).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

/// `pause` stands in for sleeping between `send --async` status reads, so
/// tests can follow a message without real delays.
fn run_with_pause(
    args: &[String],
    output: &mut impl Write,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    mut pause: impl FnMut(Duration),
) -> io::Result<()> {
    let request_id = next_request_id();
    let response = match parse(args, &request_id) {
        Ok(command) => {
            let follow = command.follow;
            let sent = execute(command, &mut *send);
            if follow && sent.ok {
                // The send receipt comes first, so a caller that stops
                // reading early still knows the message id to look up.
                write_response(output, &sent)?;
                let mut followed =
                    follow_message(&sent.id, &sent.result["message_id"], send, &mut pause);
                // Both lines answer the caller's one request id.
                followed.id.clone_from(&sent.id);
                followed
            } else {
                sent
            }
        }
        Err(message) => Response::failure(&request_id, "invalid_arguments", message),
    };
    write_response(output, &response)?;
    if response.ok {
        return Ok(());
    }
    let (code, message) = response
        .error
        .map(|error| (error.code, error.message))
        .unwrap_or_else(|| (String::new(), "Developer command failed".into()));
    Err(io::Error::other(match code.as_str() {
        "message_stalled" => CliExit {
            code: STALLED_EXIT_CODE,
            message,
        }
        .into(),
        "message_abandoned" => CliExit { code: 1, message }.into(),
        _ => Box::<dyn std::error::Error + Send + Sync>::from(message),
    }))
}

/// `send --async`: reads the message's status until every recipient has worked
/// on it and gone idle again (`turn_ended`), with no deadline. Orchestrators
/// run it as a background tool call and are woken when it exits, so a long
/// task must never time out. A blocked recipient is not idle, so it keeps
/// waiting. It fails only when a request is abandoned or the message is gone.
fn follow_message(
    id: &str,
    message: &Value,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) -> Response {
    // The send receipt carries the id as a number; message.status takes the
    // same selector text the CLI's `message status` passes.
    let message = match message {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    loop {
        let request = Request {
            id: next_request_id(),
            method: "message.status".into(),
            params: json!({ "message": message }),
        };
        match send(&request, None) {
            // The server can be briefly unreachable, e.g. while it saves;
            // keep following rather than report a failed delivery.
            Err(_) => pause(Duration::from_secs(1)),
            Ok(response) if !response.ok => return response,
            Ok(response) => match async_outcome(&response.result) {
                Some(Ok(())) => return response,
                Some(Err((code, message))) => {
                    let mut failure = Response::failure(id, code, message);
                    failure.result = response.result;
                    return failure;
                }
                None => pause(Duration::from_millis(200)),
            },
        }
    }
}

/// `Some(Ok)` once every recipient's turn ended, `Some(Err)` with an error
/// code when Bus abandoned a recipient's request or one stalled, `None` while
/// any recipient still works, is blocked, or has not started.
fn async_outcome(status: &Value) -> Option<Result<(), (&'static str, String)>> {
    let requests = status["requests"].as_array()?;
    if let Some(abandoned) = requests.iter().find(|r| r["stage"] == "abandoned") {
        return Some(Err((
            "message_abandoned",
            format!(
                "Bus abandoned message {} for agent {}; inspect message status and the agent's terminal",
                status["message_id"],
                abandoned["agent_name"].as_str().unwrap_or("unknown"),
            ),
        )));
    }
    if requests.iter().all(|r| r["turn_ended"] == true) {
        return Some(Ok(()));
    }
    // A finished turn whose reply was not captured is done for --async.
    stalled(status, |r| r["turn_ended"] != true).map(Err)
}

/// The first stalled request (among those `relevant`) as a `message_stalled`
/// error with its reason.
fn stalled(status: &Value, relevant: impl Fn(&Value) -> bool) -> Option<(&'static str, String)> {
    let request = status["requests"]
        .as_array()?
        .iter()
        .find(|r| r["stage"] == "stalled" && relevant(r))?;
    Some((
        "message_stalled",
        format!(
            "message {} stalled for agent {} while {}: {}; inspect message status, then fix the agent or run request recover",
            status["message_id"],
            request["agent_name"].as_str().unwrap_or("unknown"),
            request["stalled_from"].as_str().unwrap_or("unknown"),
            request["reason"].as_str().unwrap_or("unknown"),
        ),
    ))
}

fn timeout_response(id: &str, last_status: Value) -> Response {
    let mut response = Response::failure(
        id,
        "timeout",
        "The message did not complete before the wait deadline; inspect result for its last status",
    );
    response.result = last_status;
    response
}

/// A recipient waits on a dialog, so waiting longer would only run out the clock.
fn dialog_response(id: &str, last_status: Value) -> Response {
    let mut response = Response::failure(
        id,
        "agent_waiting_on_dialog",
        "A recipient is waiting on a dialog; use agent dialog, then agent choose or agent answer, then wait again",
    );
    response.result = last_status;
    response
}

fn execute(
    command: ParsedCommand,
    mut send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
) -> Response {
    let id = command.request_id;
    let deadline = command.wait_timeout.map(|timeout| Instant::now() + timeout);
    let mut request = Request {
        id: id.clone(),
        method: command.method.into(),
        params: command.params,
    };
    let mut last_status = Value::Null;
    loop {
        let remaining = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        if remaining == Some(Duration::ZERO) {
            return timeout_response(&id, last_status);
        }
        let mut response = match send(&request, remaining) {
            Ok(response) => response,
            Err(message) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return timeout_response(&id, last_status);
                }
                let mut response = Response::failure(&id, "transport_error", message);
                response.result = last_status;
                return response;
            }
        };
        response.id.clone_from(&id);
        if !response.ok || deadline.is_none() || response.result["complete"] == true {
            return response;
        }
        if response.result["waiting_on_dialog"]
            .as_array()
            .is_some_and(|agents| !agents.is_empty())
        {
            return dialog_response(&id, response.result);
        }
        // A stalled message will not complete by waiting longer.
        if let Some((code, message)) = stalled(&response.result, |_| true) {
            let mut failure = Response::failure(&id, code, message);
            failure.result = response.result;
            return failure;
        }
        last_status = response.result;
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            thread::sleep(Duration::from_millis(200).min(remaining));
        }
        // Every status read is fresh even if the server caches request outcomes.
        request.id = next_request_id();
    }
}

#[derive(Debug)]
struct ParsedCommand {
    method: &'static str,
    params: Value,
    request_id: String,
    wait_timeout: Option<Duration>,
    /// `send --async`: after sending, follow the message until every
    /// recipient's turn ended.
    follow: bool,
}

fn value_arg(name: &'static str) -> Arg {
    Arg::new(name).value_parser(NonEmptyStringValueParser::new())
}

fn option(name: &'static str) -> Arg {
    value_arg(name).long(name).required(true)
}

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}

fn subcommand(name: &'static str) -> Command {
    Command::new(name)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
}

fn cli() -> Command {
    subcommand("bus")
        .no_binary_name(true)
        .subcommand_required(true)
        .arg(value_arg("request-id").long("request-id").global(true))
        .subcommand(subcommand("state"))
        .subcommand(subcommand("quit"))
        .subcommand(
            subcommand("settings")
                .subcommand(toggle("color-blind"))
                .subcommand(toggle("room-sound").arg(value_arg("sound").long("sound"))),
        )
        .subcommand(subcommand("diagnostics"))
        .subcommand(subcommand("sounds"))
        .subcommand(
            subcommand("room")
                .subcommand_required(true)
                .subcommand(subcommand("create").arg(value_arg("name").required(true)))
                .subcommand(subcommand("focus").arg(value_arg("room").required(true)))
                .subcommand(subcommand("seen").arg(value_arg("room").required(true)))
                .subcommand(
                    toggle("sound")
                        .arg(value_arg("room").required(true))
                        .arg(value_arg("sound").long("sound")),
                )
                .subcommand(
                    subcommand("rename")
                        .arg(value_arg("room").required(true))
                        .arg(value_arg("name").required(true)),
                )
                .subcommand(
                    subcommand("notes")
                        .arg(value_arg("room").required(true))
                        .arg(
                            Arg::new("text")
                                .long("text")
                                .required(true)
                                .allow_hyphen_values(true),
                        ),
                )
                .subcommand(
                    subcommand("delete")
                        .arg(value_arg("room").required(true))
                        .arg(flag("confirm")),
                ),
        )
        .subcommand(
            subcommand("agent")
                .subcommand_required(true)
                .subcommand(
                    subcommand("add")
                        .arg(option("room"))
                        .arg(option("name"))
                        .arg(option("provider").value_parser(["claude", "codex", "cursor"]))
                        .arg(option("pwd"))
                        .arg(Arg::new("args").long("args").allow_hyphen_values(true))
                        .arg(flag("consent-hooks"))
                        .arg(value_arg("orchestrates").long("orchestrates"))
                        .arg(
                            Arg::new("system-prompt")
                                .long("system-prompt")
                                .allow_hyphen_values(true)
                                .conflicts_with("system-prompt-file"),
                        )
                        .arg(value_arg("system-prompt-file").long("system-prompt-file")),
                )
                .subcommand(
                    subcommand("read")
                        .arg(value_arg("agent").required(true))
                        .arg(
                            Arg::new("source")
                                .long("source")
                                .value_parser(["visible", "recent"]),
                        )
                        .arg(
                            Arg::new("lines")
                                .long("lines")
                                .value_parser(clap::value_parser!(u32).range(1..)),
                        ),
                )
                .subcommand(subcommand("dialog").arg(value_arg("agent").required(true)))
                .subcommand(
                    subcommand("choose")
                        .arg(value_arg("agent").required(true))
                        .arg(option("option"))
                        .arg(option("fingerprint")),
                )
                .subcommand(
                    subcommand("answer")
                        .arg(value_arg("agent").required(true))
                        .arg(
                            value_arg("text")
                                .long("text")
                                .required_unless_present("skip")
                                .conflicts_with("skip"),
                        )
                        .arg(flag("skip").required_unless_present("text"))
                        .arg(option("fingerprint")),
                )
                .subcommand(subcommand("focus").arg(value_arg("agent").required(true)))
                .subcommand(subcommand("clear").arg(value_arg("agent").required(true)))
                .subcommand(toggle("details").arg(value_arg("agent").required(true)))
                .subcommand(
                    subcommand("rename")
                        .arg(value_arg("agent").required(true))
                        .arg(value_arg("name").required(true)),
                )
                .subcommand(
                    subcommand("setup-confirm")
                        .arg(value_arg("agent").required(true))
                        .arg(flag("confirm")),
                )
                .subcommand(
                    subcommand("delete")
                        .arg(value_arg("agent").required(true))
                        .arg(flag("confirm")),
                ),
        )
        .subcommand(
            subcommand("send")
                .arg(option("room"))
                .arg(option("to"))
                .arg(option("text"))
                .arg(value_arg("file").long("file").action(ArgAction::Append))
                .arg(value_arg("as").long("as"))
                .arg(flag("queue"))
                .arg(flag("async")),
        )
        .subcommand(
            subcommand("message")
                .subcommand_required(true)
                .subcommand(subcommand("status").arg(value_arg("message").required(true))),
        )
        .subcommand(
            subcommand("request").subcommand_required(true).subcommand(
                subcommand("recover")
                    .arg(value_arg("request").required(true))
                    .arg(flag("confirm")),
            ),
        )
        .subcommand(
            subcommand("wait").arg(option("message")).arg(
                Arg::new("timeout")
                    .long("timeout")
                    .default_value("60")
                    .value_parser(clap::value_parser!(u64).range(1..=600)),
            ),
        )
        .subcommand(subcommand("history").arg(option("room")))
}

fn required<'a>(matches: &'a ArgMatches, name: &str) -> Result<&'a str, String> {
    matches
        .get_one::<String>(name)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing required argument: {name}"))
}

fn on_off(args: &ArgMatches, command: &str) -> Result<bool, String> {
    match (args.get_flag("on"), args.get_flag("off")) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        _ => Err(format!("{command} needs exactly one of --on or --off")),
    }
}

fn toggle(name: &'static str) -> Command {
    subcommand(name)
        .arg(flag("on").conflicts_with("off"))
        .arg(flag("off"))
}

/// Destructive commands name the missing flag instead of clap's generic error.
fn confirmed(args: &ArgMatches) -> Result<bool, String> {
    if args.get_flag("confirm") {
        Ok(true)
    } else {
        Err("This command changes or removes work; add --confirm to proceed".into())
    }
}

fn parse(args: &[String], request_id: &str) -> Result<ParsedCommand, String> {
    let matches = cli()
        .try_get_matches_from(args)
        .map_err(|error| error.to_string())?;
    let request_id = matches
        .get_one::<String>("request-id")
        .map(String::as_str)
        .unwrap_or(request_id)
        .to_owned();
    if request_id.trim().is_empty() {
        return Err("request-id must not be blank".into());
    }
    let (name, args) = matches
        .subcommand()
        .ok_or_else(|| "a developer command is required".to_owned())?;
    let mut wait_timeout = None;
    let mut follow = false;
    let (method, params) = match name {
        "state" => ("state", json!({})),
        "quit" => ("bus.quit", json!({})),
        "settings" => match args.subcommand() {
            None => ("settings", json!({})),
            Some(("color-blind", args)) => (
                "settings.color_blind",
                json!({"on": on_off(args, "settings color-blind")?}),
            ),
            Some(("room-sound", args)) => {
                let mut params = json!({"on": on_off(args, "settings room-sound")?});
                if args.contains_id("sound") {
                    params["sound"] = required(args, "sound")?.into();
                }
                ("settings.room_sound", params)
            }
            _ => return Err("unknown settings command".into()),
        },
        "diagnostics" => ("diagnostics", json!({})),
        "sounds" => ("sounds", json!({})),
        "room" => match args.subcommand() {
            Some(("focus", args)) => ("room.focus", json!({"room": required(args, "room")?})),
            Some(("seen", args)) => ("room.seen", json!({"room": required(args, "room")?})),
            Some(("sound", args)) => {
                let mut params =
                    json!({"room": required(args, "room")?, "on": on_off(args, "room sound")?});
                if args.contains_id("sound") {
                    params["sound"] = required(args, "sound")?.into();
                }
                ("room.sound", params)
            }
            Some(("create", args)) => ("room.create", json!({"name": required(args, "name")?})),
            Some(("rename", args)) => (
                "room.rename",
                json!({"room": required(args, "room")?, "name": required(args, "name")?}),
            ),
            Some(("notes", args)) => (
                "room.notes",
                json!({
                    "room": required(args, "room")?,
                    "text": args.get_one::<String>("text").map(String::as_str).unwrap(),
                }),
            ),
            Some(("delete", args)) => (
                "room.delete",
                json!({"room": required(args, "room")?, "confirm": confirmed(args)?}),
            ),
            _ => return Err("unknown room command".into()),
        },
        "agent" => match args.subcommand() {
            Some(("focus", args)) => ("agent.focus", json!({"agent": required(args, "agent")?})),
            Some(("clear", args)) => ("agent.clear", json!({"agent": required(args, "agent")?})),
            Some(("details", args)) => (
                "agent.details",
                json!({"agent": required(args, "agent")?, "on": on_off(args, "agent details")?}),
            ),
            Some(("rename", args)) => (
                "agent.rename",
                json!({"agent": required(args, "agent")?, "name": required(args, "name")?}),
            ),
            Some(("add", args)) => {
                let mut params = json!({
                    "room": required(args, "room")?, "name": required(args, "name")?,
                    "provider": required(args, "provider")?, "cwd": required(args, "pwd")?,
                    "extra_args": args.get_one::<String>("args").map(String::as_str).unwrap_or(""),
                    "consent_project_hooks": args.get_flag("consent-hooks"),
                });
                if let Some(room) = args.get_one::<String>("orchestrates") {
                    params["orchestrates"] = json!(room);
                }
                if let Some(text) = args.get_one::<String>("system-prompt") {
                    params["system_prompt"] = json!(text);
                }
                if let Some(path) = args.get_one::<String>("system-prompt-file") {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| format!("Cannot read --system-prompt-file {path}: {e}"))?;
                    params["system_prompt"] = json!(text);
                }
                ("agent.add", params)
            }
            Some(("read", args)) => {
                let mut params = json!({"agent": required(args, "agent")?});
                let source = args.get_one::<String>("source").map(String::as_str);
                let lines = args.get_one::<u32>("lines").copied();
                match (source, lines) {
                    (Some("visible"), None) => params["source"] = json!("visible"),
                    (Some("visible"), Some(_)) => {
                        return Err(
                            "Visible reads return the complete viewport; omit --lines".into()
                        )
                    }
                    (None | Some("recent"), Some(lines)) => {
                        params["source"] = json!("recent");
                        params["lines"] = json!(lines);
                    }
                    (None | Some("recent"), None) => {
                        return Err("Recent reads require an explicit positive --lines N".into())
                    }
                    (Some(_), _) => return Err("Unknown terminal read source".into()),
                }
                ("agent.read", params)
            }
            Some(("dialog", args)) => (
                "agent.dialog.observe",
                json!({"agent": required(args, "agent")?}),
            ),
            Some(("choose", args)) => (
                "agent.dialog.choose",
                json!({
                    "agent": required(args, "agent")?,
                    "option": required(args, "option")?,
                    "fingerprint": required(args, "fingerprint")?,
                }),
            ),
            Some(("answer", args)) => (
                "agent.dialog.answer",
                json!({"agent": required(args, "agent")?, "text":if args.get_flag("skip") {None} else {Some(required(args,"text")?)},
                    "skip":args.get_flag("skip"), "fingerprint":required(args,"fingerprint")?}),
            ),
            Some(("setup-confirm", args)) => (
                "agent.setup-confirm",
                json!({"agent": required(args, "agent")?, "confirm": confirmed(args)?}),
            ),
            Some(("delete", args)) => (
                "agent.delete",
                json!({"agent": required(args, "agent")?, "confirm": confirmed(args)?}),
            ),
            _ => return Err("unknown agent command".into()),
        },
        "send" => {
            let recipients: Vec<_> = required(args, "to")?.split(',').collect();
            if recipients
                .iter()
                .any(|recipient| recipient.trim().is_empty())
            {
                return Err(
                    "--to requires a nonempty selector in every comma-separated entry".into(),
                );
            }
            let mut params = json!({
                "room": required(args, "room")?, "to": recipients,
                "text": required(args, "text")?,
                "files": args.get_many::<String>("file").map(|values| values.cloned().collect::<Vec<_>>()).unwrap_or_default(),
            });
            // Omitted rather than defaulted so Human sends keep their request shape.
            if args.contains_id("as") {
                params["as"] = json!(required(args, "as")?);
            }
            if args.get_flag("queue") {
                params["queue"] = json!(true);
            }
            follow = args.get_flag("async");
            if follow
                && recipients
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(crate::messaging::model::HUMAN_RECIPIENT))
            {
                return Err("--async waits for agents; a message to the human has none".into());
            }
            ("message.send", params)
        }
        "message" => match args.subcommand() {
            Some(("status", args)) => (
                "message.status",
                json!({"message": required(args, "message")?}),
            ),
            _ => return Err("unknown message command".into()),
        },
        "request" => match args.subcommand() {
            Some(("recover", args)) => (
                "request.recover",
                json!({"request": required(args, "request")?, "confirm": confirmed(args)?}),
            ),
            _ => return Err("unknown request command".into()),
        },
        "wait" => {
            let timeout = args
                .get_one::<u64>("timeout")
                .copied()
                .ok_or_else(|| "missing wait timeout".to_owned())?;
            wait_timeout = Some(Duration::from_secs(timeout));
            (
                "message.status",
                json!({"message": required(args, "message")?}),
            )
        }
        "history" => ("room.history", json!({"room": required(args, "room")?})),
        _ => return Err("unknown developer command".into()),
    };
    Ok(ParsedCommand {
        method,
        params,
        request_id,
        wait_timeout,
        follow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(args: &[&str]) -> Result<ParsedCommand, String> {
        parse(
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            "generated-request",
        )
    }

    #[test]
    fn agent_add_reads_the_system_prompt_file() {
        let path = std::env::temp_dir().join(format!("bus-prompt-{}.md", std::process::id()));
        std::fs::write(&path, "You run {{ROOM_NAME}}.\n").unwrap();
        let parsed = command(&[
            "agent",
            "add",
            "--room",
            "master",
            "--name",
            "orch",
            "--provider",
            "cursor",
            "--pwd",
            "/repo",
            "--orchestrates",
            "pr-1",
            "--system-prompt-file",
            path.to_str().unwrap(),
        ])
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(parsed.params["system_prompt"], "You run {{ROOM_NAME}}.\n");
        assert_eq!(parsed.params["orchestrates"], "pr-1");
        assert_eq!(parsed.params["cwd"], "/repo");
    }

    #[test]
    fn send_preserves_explicit_recipients_text_files_and_request_identity() {
        let parsed = command(&[
            "send",
            "--room",
            "planning",
            "--to",
            "codex,17,Claude Agent",
            "--text",
            "Review the changes\nThen report back.",
            "--file",
            "/tmp/one patch.diff",
            "--file",
            "/tmp/two.png",
            "--request-id",
            "submission-123",
        ])
        .unwrap();
        assert_eq!(parsed.method, "message.send");
        assert_eq!(
            parsed.params,
            json!({
                "room": "planning", "to": ["codex", "17", "Claude Agent"],
                "text": "Review the changes\nThen report back.",
                "files": ["/tmp/one patch.diff", "/tmp/two.png"]
            })
        );
        assert_eq!(parsed.request_id, "submission-123");
        assert!(parsed.wait_timeout.is_none());
    }

    #[test]
    fn room_sound_requires_exactly_one_of_on_or_off() {
        for args in [
            &["room", "sound", "7"][..],
            &["room", "sound", "7", "--on", "--off"][..],
            &["room", "sound", "--on"][..],
            &["room", "sound", "7", "8", "--on"][..],
            &["room", "sound", "7", "--sound", "Glass"][..],
            &["room", "sound", "7", "--on", "--sound"][..],
            &["room", "sound", "7", "--on", "--sound", " "][..],
        ] {
            assert!(command(args).is_err(), "{args:?}");
        }
        assert!(HELP.contains("room sound ROOM (--on | --off) [--sound NAME]"));
        assert!(HELP.contains("settings room-sound (--on | --off) [--sound NAME]"));
        assert!(HELP.contains("\n  sounds\n"));
    }

    #[test]
    fn orchestrators_have_no_reassign_command() {
        assert!(command(&["agent", "orchestrate", "claude-orch", "--none"]).is_err());
        assert!(!HELP.contains("agent orchestrate AGENT"));
        assert!(HELP.contains("exactly one work room for its whole life"));
        assert!(HELP.contains("[--orchestrates ROOM]"));
        assert!(HELP.contains("[--system-prompt TEXT | --system-prompt-file PATH]"));
    }

    #[test]
    fn toggles_require_exactly_one_of_on_or_off() {
        for args in [
            &["agent", "details", "2"][..],
            &["settings", "color-blind"],
            &["settings", "room-sound", "--sound", "Glass"],
            &["agent", "details", "2", "--on", "--off"],
        ] {
            assert!(command(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn destructive_commands_without_confirm_name_the_flag() {
        for args in [
            &["room", "delete", "planning"][..],
            &["agent", "delete", "2"],
            &["agent", "setup-confirm", "2"],
            &["request", "recover", "64"],
        ] {
            let error = command(args).unwrap_err();
            assert!(error.contains("--confirm"), "{args:?}: {error}");
        }
    }

    #[test]
    fn commands_route_to_exact_methods_with_string_selectors() {
        let cases: &[(&[&str], &str, Value)] = &[
            (&["state"], "state", json!({})),
            (&["diagnostics"], "diagnostics", json!({})),
            (
                &["room", "create", "Design Room"],
                "room.create",
                json!({"name": "Design Room"}),
            ),
            (
                &["room", "rename", "7", "Planning"],
                "room.rename",
                json!({"room": "7", "name": "Planning"}),
            ),
            (
                &["room", "notes", "Planning", "--text", "Goal\n- ship notes"],
                "room.notes",
                json!({"room": "Planning", "text": "Goal\n- ship notes"}),
            ),
            (
                &["room", "notes", "7", "--text", ""],
                "room.notes",
                json!({"room": "7", "text": ""}),
            ),
            (
                &["room", "delete", "planning", "--confirm"],
                "room.delete",
                json!({"room": "planning", "confirm": true}),
            ),
            (
                &["agent", "read", "Claude Agent", "--source", "visible"],
                "agent.read",
                json!({"agent": "Claude Agent", "source": "visible"}),
            ),
            (
                &[
                    "agent",
                    "read",
                    "Claude Agent",
                    "--source",
                    "recent",
                    "--lines",
                    "80",
                ],
                "agent.read",
                json!({"agent": "Claude Agent", "source": "recent", "lines": 80}),
            ),
            (
                &[
                    "agent",
                    "read",
                    "Claude Agent",
                    "--source",
                    "recent",
                    "--lines",
                    "5000",
                ],
                "agent.read",
                json!({"agent": "Claude Agent", "source": "recent", "lines": 5000}),
            ),
            (
                &["agent", "read", "Claude Agent", "--lines", "50"],
                "agent.read",
                json!({"agent": "Claude Agent", "source": "recent", "lines": 50}),
            ),
            (
                &[
                    "agent",
                    "add",
                    "--room",
                    "master",
                    "--name",
                    "claude-orch",
                    "--provider",
                    "claude",
                    "--pwd",
                    "/repo",
                    "--orchestrates",
                    "pr-123",
                ],
                "agent.add",
                json!({
                    "room": "master", "name": "claude-orch", "provider": "claude",
                    "cwd": "/repo", "extra_args": "", "consent_project_hooks": false,
                    "orchestrates": "pr-123"
                }),
            ),
            (
                &[
                    "agent",
                    "add",
                    "--room",
                    "master",
                    "--name",
                    "codex-orch",
                    "--provider",
                    "codex",
                    "--pwd",
                    "/repo",
                    "--system-prompt",
                    "Run {{ROOM_NAME}}.",
                ],
                "agent.add",
                json!({
                    "room": "master", "name": "codex-orch", "provider": "codex",
                    "cwd": "/repo", "extra_args": "", "consent_project_hooks": false,
                    "system_prompt": "Run {{ROOM_NAME}}."
                }),
            ),
            (
                &["room", "sound", "master", "--off"],
                "room.sound",
                json!({"room": "master", "on": false}),
            ),
            (
                &["room", "sound", "7", "--on"],
                "room.sound",
                json!({"room": "7", "on": true}),
            ),
            (
                &["room", "sound", "7", "--on", "--sound", "Windows Notify"],
                "room.sound",
                json!({"room": "7", "on": true, "sound": "Windows Notify"}),
            ),
            (&["sounds"], "sounds", json!({})),
            (&["quit"], "bus.quit", json!({})),
            (
                &["settings", "color-blind", "--on"],
                "settings.color_blind",
                json!({"on": true}),
            ),
            (&["settings"], "settings", json!({})),
            (
                &["settings", "room-sound", "--on", "--sound", "Glass"],
                "settings.room_sound",
                json!({"on": true, "sound": "Glass"}),
            ),
            (
                &["settings", "room-sound", "--off"],
                "settings.room_sound",
                json!({"on": false}),
            ),
            (
                &["agent", "details", "2", "--off"],
                "agent.details",
                json!({"agent": "2", "on": false}),
            ),
            (
                &["room", "seen", "Planning"],
                "room.seen",
                json!({"room": "Planning"}),
            ),
            (
                &["agent", "rename", "2", "Code Reviewer"],
                "agent.rename",
                json!({"agent": "2", "name": "Code Reviewer"}),
            ),
            (
                &[
                    "send", "--room", "7", "--to", "all", "--text", "hi", "--as", "codex1",
                ],
                "message.send",
                json!({"room": "7", "to": ["all"], "text": "hi", "files": [], "as": "codex1"}),
            ),
            (
                &["agent", "setup-confirm", "2", "--confirm"],
                "agent.setup-confirm",
                json!({"agent": "2", "confirm": true}),
            ),
            (
                &["agent", "delete", "2", "--confirm"],
                "agent.delete",
                json!({"agent": "2", "confirm": true}),
            ),
            (
                &["message", "status", "19"],
                "message.status",
                json!({"message": "19"}),
            ),
            (
                &["request", "recover", "64", "--confirm"],
                "request.recover",
                json!({"request": "64", "confirm": true}),
            ),
            (
                &["history", "--room", "Planning"],
                "room.history",
                json!({"room": "Planning"}),
            ),
        ];
        for (args, method, params) in cases {
            let parsed = command(args).unwrap_or_else(|error| panic!("{args:?}: {error}"));
            assert_eq!(parsed.method, *method, "{args:?}");
            assert_eq!(parsed.params, *params, "{args:?}");
            assert_eq!(parsed.request_id, "generated-request", "{args:?}");
            assert!(parsed.wait_timeout.is_none(), "{args:?}");
        }
    }

    #[test]
    fn agent_add_maps_launch_options_and_explicit_hook_consent() {
        for provider in ["claude", "codex", "cursor"] {
            let parsed = command(&[
                "agent",
                "add",
                "--room",
                "7",
                "--name",
                "Reviewer",
                "--provider",
                provider,
                "--pwd",
                "/tmp/work project",
                "--args",
                "--model fast --verbose",
                "--consent-hooks",
            ])
            .unwrap();
            assert_eq!(parsed.method, "agent.add");
            assert_eq!(
                parsed.params,
                json!({
                    "room": "7", "name": "Reviewer", "provider": provider,
                    "cwd": "/tmp/work project", "extra_args": "--model fast --verbose",
                    "consent_project_hooks": true,
                })
            );
        }
        let parsed = command(&[
            "agent",
            "add",
            "--room",
            "7",
            "--name",
            "Reviewer",
            "--provider",
            "codex",
            "--pwd",
            "/tmp",
        ])
        .unwrap();
        assert_eq!(parsed.params["extra_args"], "");
        assert_eq!(parsed.params["consent_project_hooks"], false);
    }

    #[test]
    fn room_orchestrator_core_cli_wait_is_a_bounded_message_status_query() {
        let parsed = command(&["wait", "--message", "19", "--timeout", "600"]).unwrap();
        assert_eq!(parsed.method, "message.status");
        assert_eq!(parsed.params, json!({"message": "19"}));
        assert_eq!(parsed.wait_timeout, Some(Duration::from_secs(600)));
        let default = command(&["wait", "--message", "19"]).unwrap();
        assert_eq!(default.wait_timeout, Some(Duration::from_secs(60)));
    }

    #[test]
    fn reserved_all_recipient_is_explicit_and_never_implicit() {
        let parsed = command(&["send", "--room", "7", "--to", "all", "--text", "hello"]).unwrap();
        assert_eq!(parsed.params["to"], json!(["all"]));
        assert_eq!(parsed.params["files"], json!([]));
        assert!(command(&["send", "--room", "7", "--text", "hello"]).is_err());
    }

    #[test]
    fn invalid_or_ambiguous_commands_do_not_create_requests() {
        let cases: &[&[&str]] = &[
            &[],
            &["unknown"],
            &["state", "--unknown"],
            &["state", "extra"],
            &["room", "create"],
            &["room", "rename", "7"],
            &["room", "notes", "7"],
            &["room", "delete", "7"],
            &["agent", "delete", "9"],
            &["agent", "setup-confirm", "9"],
            &["agent", "read", "Claude Agent"],
            &["agent", "read", "Claude Agent", "--source", "recent"],
            &["agent", "read", "Claude Agent", "--source", "detection"],
            &[
                "agent",
                "read",
                "Claude Agent",
                "--source",
                "visible",
                "--lines",
                "20",
            ],
            &[
                "agent",
                "add",
                "--room",
                "7",
                "--name",
                "Reviewer",
                "--provider",
                "invalid",
                "--pwd",
                "/tmp",
            ],
            &[
                "agent",
                "add",
                "--room",
                "7",
                "--name",
                "Reviewer",
                "--provider",
                "codex",
            ],
            &[
                "agent",
                "add",
                "--room",
                "master",
                "--name",
                "orch",
                "--provider",
                "codex",
                "--pwd",
                "/tmp",
                "--system-prompt",
                "x",
                "--system-prompt-file",
                "/tmp/x",
            ],
            &[
                "agent",
                "add",
                "--room",
                "master",
                "--name",
                "orch",
                "--provider",
                "codex",
                "--pwd",
                "/tmp",
                "--system-prompt-file",
                "/nonexistent/bus-prompt.md",
            ],
            &["send", "--room", "7", "--to", "9"],
            &["send", "--room", "7", "--to", "9", "--text"],
            &[
                "send", "--room", "7", "--to", "9", "--text", "hello", "--room", "8",
            ],
            &["wait", "--message", "19", "--timeout", "0"],
            &["wait", "--message", "19", "--timeout", "601"],
            &["wait", "--message", "19", "--timeout", "NaN"],
            &["history"],
            &["message", "status"],
            &["request", "recover", "64"],
            &["state", "--request-id", "a", "--request-id", "b"],
        ];
        for args in cases {
            assert!(command(args).is_err(), "must reject {args:?}");
        }
    }

    #[test]
    fn room_orchestrator_core_bus_read_forms_require_model_selected_evidence_bounds() {
        let visible = command(&["agent", "read", "Reviewer", "--source", "visible"]).unwrap();
        assert_eq!(
            visible.params,
            json!({"agent":"Reviewer","source":"visible"})
        );
        let recent = command(&["agent", "read", "Reviewer", "--lines", "5001"]).unwrap();
        assert_eq!(
            recent.params,
            json!({"agent":"Reviewer","source":"recent","lines":5001})
        );
        assert!(command(&["agent", "read", "Reviewer"]).is_err());
        assert!(command(&["agent", "read", "Reviewer", "--source", "recent"]).is_err());
        assert!(
            command(&["agent", "read", "Reviewer", "--source", "visible", "--lines", "2"]).is_err()
        );
    }

    #[test]
    fn agent_dialog_cli_observes_and_chooses_with_a_fingerprint() {
        let observe = command(&["agent", "dialog", "Reviewer"]).unwrap();
        assert_eq!(observe.method, "agent.dialog.observe");
        assert_eq!(observe.params, json!({"agent":"Reviewer"}));
        let choose = command(&[
            "agent",
            "choose",
            "Reviewer",
            "--option",
            "2",
            "--fingerprint",
            "d1.abc.def",
        ])
        .unwrap();
        assert_eq!(choose.method, "agent.dialog.choose");
        assert_eq!(
            choose.params,
            json!({"agent":"Reviewer","option":"2","fingerprint":"d1.abc.def"})
        );
        for args in [
            &["agent", "choose", "Reviewer", "--option", "2"][..],
            &["agent", "choose", "Reviewer", "--fingerprint", "d1.abc.def"],
            &[
                "agent",
                "choose",
                "Reviewer",
                "--option",
                " ",
                "--fingerprint",
                "f",
            ],
            &["agent", "dialog", "Reviewer", "--keys", "enter"],
            &["agent", "permission", "Reviewer"],
            &["agent", "approve-once", "Reviewer", "--fingerprint", "f"],
        ] {
            assert!(command(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn agent_answer_requires_a_fingerprint_and_exactly_one_answer_mode() {
        let text = command(&[
            "agent",
            "answer",
            "61",
            "--text",
            "MY TOKEN",
            "--fingerprint",
            "f",
        ])
        .unwrap();
        assert_eq!(text.method, "agent.dialog.answer");
        assert_eq!(
            text.params,
            json!({"agent":"61","text":"MY TOKEN","skip":false,"fingerprint":"f"})
        );
        let skip = command(&["agent", "answer", "61", "--skip", "--fingerprint", "f"]).unwrap();
        assert_eq!(
            skip.params,
            json!({"agent":"61","text":null,"skip":true,"fingerprint":"f"})
        );
        for args in [
            vec!["agent", "answer", "61", "--text", "ok"],
            vec!["agent", "answer", "61", "--fingerprint", "f"],
            vec![
                "agent",
                "answer",
                "61",
                "--text",
                "ok",
                "--skip",
                "--fingerprint",
                "f",
            ],
            vec!["agent", "answer", "61", "--text", " ", "--fingerprint", "f"],
            vec![
                "agent",
                "answer",
                "61",
                "--skip",
                "--keys",
                "enter",
                "--fingerprint",
                "f",
            ],
        ] {
            assert!(command(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn blank_identity_or_recipient_entries_cannot_be_dispatched() {
        let cases: &[&[&str]] = &[
            &["room", "create", "   "],
            &["agent", "read", "\t"],
            &["state", "--request-id", "   "],
            &["send", "--room", "7", "--to", "one,,two", "--text", "hello"],
            &[
                "send",
                "--room",
                "7",
                "--to",
                "one, ,two",
                "--text",
                "hello",
            ],
            &["send", "--room", "7", "--to", "one,", "--text", "hello"],
        ];
        for args in cases {
            assert!(command(args).is_err(), "must reject {args:?}");
        }
    }

    fn run_captured(
        args: &[&str],
        send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    ) -> (io::Result<()>, Value) {
        let mut output = Vec::new();
        let outcome = run_with(
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            &mut output,
            send,
        );
        let output = String::from_utf8(output).unwrap();
        assert_eq!(
            output.lines().count(),
            1,
            "exactly one JSON response: {output:?}"
        );
        (outcome, serde_json::from_str(&output).unwrap())
    }

    /// One recipient's status in a `message.status` result.
    fn recipient(agent: u64, stage: &str, turn_ended: bool) -> Value {
        json!({"request_id": 40 + agent, "agent_id": agent, "agent_name": format!("a{agent}"),
            "stage": stage, "turn_ended": turn_ended})
    }

    fn status(requests: Vec<Value>) -> Value {
        json!({"message_id": 19, "complete": false, "requests": requests})
    }

    /// Runs `send --async` against scripted statuses; returns the outcome,
    /// every printed JSON line, the status reads' timeouts, and the pauses.
    fn run_async(
        extra: &[&str],
        statuses: impl IntoIterator<Item = Value>,
    ) -> (
        io::Result<()>,
        Vec<Value>,
        Vec<Option<Duration>>,
        Vec<Duration>,
    ) {
        let mut statuses = statuses.into_iter();
        let mut timeouts = Vec::new();
        let mut pauses = Vec::new();
        let mut output = Vec::new();
        let mut args = vec![
            "send", "--room", "7", "--to", "a1", "--text", "go", "--async",
        ];
        args.extend_from_slice(extra);
        let outcome = run_with_pause(
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            &mut output,
            &mut |request: &Request, timeout: Option<Duration>| {
                if request.method == "message.send" {
                    return Ok(Response::success(
                        &request.id,
                        json!({"message_id": 19, "request_ids": [41], "stage": "queued"}),
                    ));
                }
                assert_eq!(request.method, "message.status");
                assert_eq!(request.params, json!({"message": "19"}));
                timeouts.push(timeout);
                Ok(Response::success(
                    &request.id,
                    statuses.next().expect("polled past the last status"),
                ))
            },
            |pause| pauses.push(pause),
        );
        let lines = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (outcome, lines, timeouts, pauses)
    }

    #[test]
    fn send_async_prints_the_message_id_then_returns_once_the_turn_ended() {
        let (outcome, lines, timeouts, pauses) = run_async(
            &[],
            [
                status(vec![recipient(1, "queued", false)]),
                status(vec![recipient(1, "delivered", false)]),
                status(vec![recipient(1, "delivered", true)]),
            ],
        );
        assert!(outcome.is_ok());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["result"]["message_id"], 19);
        assert_eq!(lines[1]["ok"], true);
        assert_eq!(lines[1]["result"]["requests"][0]["turn_ended"], true);
        assert_eq!(lines[0]["id"], lines[1]["id"]);
        assert!(timeouts.iter().all(Option::is_none), "no per-read deadline");
        assert_eq!(pauses, [Duration::from_millis(200); 2]);
    }

    #[test]
    fn send_async_has_no_time_limit_and_keeps_waiting_while_blocked() {
        // About 33 minutes of polling, far past wait's 600-second cap.
        let polls = 10_000;
        let statuses = (0..polls)
            .map(|_| status(vec![recipient(1, "delivered", false)]))
            .chain([status(vec![recipient(1, "delivered", true)])]);
        let (outcome, lines, _, pauses) = run_async(&[], statuses);
        assert!(outcome.is_ok());
        assert_eq!(lines.len(), 2);
        assert_eq!(pauses.len(), polls);
        assert!(pauses.iter().sum::<Duration>() > Duration::from_secs(600));
    }

    #[test]
    fn send_async_waits_for_every_recipient() {
        let (outcome, lines, _, pauses) = run_async(
            &[],
            [
                status(vec![
                    recipient(1, "delivered", true),
                    recipient(2, "queued", false),
                ]),
                status(vec![
                    recipient(1, "delivered", true),
                    recipient(2, "delivered", false),
                ]),
                status(vec![
                    recipient(1, "replied", true),
                    recipient(2, "delivered", true),
                ]),
            ],
        );
        assert!(outcome.is_ok());
        assert_eq!(pauses.len(), 2);
        assert_eq!(lines[1]["result"]["requests"][1]["turn_ended"], true);
    }

    #[test]
    fn send_async_fails_when_a_request_is_abandoned() {
        let (outcome, lines, _, _) = run_async(
            &[],
            [
                status(vec![
                    recipient(1, "delivered", false),
                    recipient(2, "delivered", false),
                ]),
                status(vec![
                    recipient(1, "delivered", true),
                    recipient(2, "abandoned", false),
                ]),
            ],
        );
        let error = outcome.unwrap_err().to_string();
        assert!(
            error.contains("abandoned message 19 for agent a2"),
            "{error}"
        );
        assert_eq!(lines[1]["ok"], false);
        assert_eq!(lines[1]["error"]["code"], "message_abandoned");
        assert_eq!(lines[1]["result"]["requests"][1]["stage"], "abandoned");
    }

    fn stalled_recipient(agent: u64, from: &str, reason: &str, turn_ended: bool) -> Value {
        json!({"request_id": 40 + agent, "agent_id": agent, "agent_name": format!("a{agent}"),
            "stage": "stalled", "stalled_from": from, "reason": reason, "turn_ended": turn_ended})
    }

    fn exit_code(outcome: &io::Result<()>) -> Option<i32> {
        outcome
            .as_ref()
            .err()?
            .get_ref()?
            .downcast_ref::<CliExit>()
            .map(|exit| exit.code)
    }

    #[test]
    fn send_async_exits_early_with_the_stalled_code_and_reason() {
        let (outcome, lines, _, _) = run_async(
            &[],
            [
                status(vec![recipient(1, "queued", false)]),
                status(vec![stalled_recipient(
                    1,
                    "queued",
                    "input_box_not_empty",
                    false,
                )]),
            ],
        );
        assert_eq!(exit_code(&outcome), Some(STALLED_EXIT_CODE));
        let error = outcome.unwrap_err().to_string();
        assert!(
            error.contains("message 19 stalled for agent a1 while queued: input_box_not_empty"),
            "{error}"
        );
        assert_eq!(lines[1]["error"]["code"], "message_stalled");
    }

    #[test]
    fn send_async_counts_a_finished_turn_without_a_captured_reply_as_done() {
        let (outcome, lines, _, _) = run_async(
            &[],
            [status(vec![stalled_recipient(
                1,
                "delivered",
                "transcript_unmatched",
                true,
            )])],
        );
        assert!(outcome.is_ok());
        assert_eq!(lines[1]["ok"], true);
    }

    #[test]
    fn an_abandoned_async_send_exits_one_with_a_clear_reason() {
        let (outcome, _, _, _) = run_async(&[], [status(vec![recipient(1, "abandoned", false)])]);
        assert_eq!(exit_code(&outcome), Some(1));
    }

    #[test]
    fn wait_exits_early_when_a_recipient_stalls() {
        let mut output = Vec::new();
        let outcome = run_with(
            &["wait", "--message", "19", "--timeout", "600"].map(String::from),
            &mut output,
            |request, _| {
                Ok(Response::success(
                    &request.id,
                    status(vec![stalled_recipient(
                        1,
                        "awaiting_start",
                        "no_start_hook",
                        false,
                    )]),
                ))
            },
        );
        assert_eq!(exit_code(&outcome), Some(STALLED_EXIT_CODE));
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], "message_stalled");
        assert_eq!(response["result"]["requests"][0]["reason"], "no_start_hook");
    }

    #[test]
    fn send_async_rejects_a_message_to_the_human() {
        let parsed = command(&[
            "send", "--room", "master", "--to", "human", "--text", "x", "--async",
        ]);
        assert!(parsed.unwrap_err().contains("--async waits for agents"));
        assert!(
            command(&["send", "--room", "7", "--to", "a1", "--text", "x", "--async"])
                .unwrap()
                .follow
        );
        assert!(HELP.contains("[--queue] [--async]"));
    }

    #[test]
    fn rejected_cli_input_prints_json_and_never_contacts_a_server() {
        let (outcome, response) = run_captured(&["room", "delete", "7"], |_, _| {
            panic!("invalid commands must not contact a server")
        });
        assert!(outcome.is_err());
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "invalid_arguments");
    }

    #[test]
    fn mutation_is_dispatched_once_and_remote_rejection_is_nonzero() {
        let mut calls = 0;
        let (outcome, response) = run_captured(
            &[
                "room",
                "create",
                "Planning",
                "--request-id",
                "room-creation-1",
            ],
            |request, timeout| {
                calls += 1;
                assert_eq!(request.id, "room-creation-1");
                assert_eq!(request.method, "room.create");
                assert_eq!(request.params, json!({"name": "Planning"}));
                assert!(timeout.is_none());
                Ok(Response::failure(
                    &request.id,
                    "already_exists",
                    "A room with that name exists",
                ))
            },
        );
        assert_eq!(calls, 1);
        assert!(outcome.is_err());
        assert_eq!(
            response,
            json!({
                "id": "room-creation-1", "ok": false, "result": null,
                "error": {"code": "already_exists", "message": "A room with that name exists"},
            })
        );
    }

    #[test]
    fn transport_failure_is_reported_once_without_retrying_mutation() {
        let mut calls = 0;
        let (outcome, response) = run_captured(
            &[
                "room",
                "create",
                "Planning",
                "--request-id",
                "room-creation-1",
            ],
            |_, _| {
                calls += 1;
                Err("No developer endpoint is available".into())
            },
        );
        assert_eq!(calls, 1);
        assert!(outcome.is_err());
        assert_eq!(response["id"], "room-creation-1");
        assert_eq!(response["error"]["code"], "transport_error");
    }

    #[test]
    fn room_orchestrator_core_cli_wait_polls_the_same_status_until_correlated_completion() {
        let statuses = [
            json!({"message_id": 19, "complete": false, "requests": [{
                "request_id": 41, "agent_id": 7, "stage": "queued", "reason": null, "reply": null,
            }]}),
            json!({"message_id": 19, "complete": true, "requests": [{
                "request_id": 41, "agent_id": 7, "stage": "completed", "reason": null, "reply": "Done",
            }]}),
        ];
        let mut calls = Vec::new();
        let (outcome, response) = run_captured(
            &[
                "wait",
                "--message",
                "19",
                "--timeout",
                "2",
                "--request-id",
                "wait-19",
            ],
            |request, timeout| {
                assert_eq!(request.method, "message.status");
                assert_eq!(request.params, json!({"message": "19"}));
                assert!(timeout.is_some_and(|value| value <= Duration::from_secs(2)));
                let result = statuses[calls.len()].clone();
                calls.push(request.id.clone());
                Ok(Response::success(&request.id, result))
            },
        );
        assert!(outcome.is_ok());
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0], calls[1]);
        assert_eq!(response["id"], "wait-19");
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"], statuses[1]);
    }

    #[test]
    fn room_orchestrator_core_cli_wait_timeout_retains_last_durable_status() {
        let status = json!({"message_id": 19, "complete": false, "requests": [{
            "request_id": 41, "agent_id": 7, "stage": "queued", "reason": "agent busy", "reply": null,
        }]});
        let mut calls = 0;
        let (outcome, response) = run_captured(
            &[
                "wait",
                "--message",
                "19",
                "--timeout",
                "1",
                "--request-id",
                "wait-19",
            ],
            |request, timeout| {
                calls += 1;
                assert!(timeout.is_some_and(|value| value <= Duration::from_secs(1)));
                Ok(Response::success(&request.id, status.clone()))
            },
        );
        assert!(calls >= 2, "pending status must be polled again");
        assert!(outcome.is_err());
        assert_eq!(response["id"], "wait-19");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "timeout");
        assert_eq!(response["result"], status);
    }

    #[test]
    fn send_queue_asks_for_an_own_turn_and_is_omitted_otherwise() {
        let queued =
            command(&["send", "--room", "r", "--to", "a", "--text", "t", "--queue"]).unwrap();
        assert_eq!(queued.params["queue"], true);
        let steering = command(&["send", "--room", "r", "--to", "a", "--text", "t"]).unwrap();
        assert!(steering.params.get("queue").is_none());
        assert!(HELP.contains("[--as AGENT] [--queue]"));
    }

    #[test]
    fn wait_returns_as_soon_as_a_recipient_waits_on_a_dialog() {
        let pending =
            json!({"message_id": 19, "complete": false, "waiting_on_dialog": [], "requests": []});
        let blocked = json!({"message_id": 19, "complete": false, "waiting_on_dialog": [7], "requests": [{
            "request_id": 41, "agent_id": 7, "stage": "delivered", "dialog": true, "reply": null,
        }]});
        let mut calls = 0;
        let (outcome, response) = run_captured(
            &["wait", "--message", "19", "--timeout", "600"],
            |request, _| {
                calls += 1;
                let status = if calls == 1 { &pending } else { &blocked };
                Ok(Response::success(&request.id, status.clone()))
            },
        );
        assert_eq!(calls, 2);
        assert!(outcome.is_err());
        assert_eq!(response["error"]["code"], "agent_waiting_on_dialog");
        assert_eq!(response["result"], blocked);
    }
}
