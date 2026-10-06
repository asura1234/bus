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

use super::control::{self, Request, Response};

#[cfg(test)]
#[path = "control_focus_cli_tests.rs"]
mod focus_tests;

pub const HELP: &str = "Developer commands (require an already running Bus --dev instance):
  state
  room create NAME
  room rename ROOM NAME
  room notes ROOM --text TEXT
  room delete ROOM --confirm
  room focus ROOM
  room seen ROOM
  room sound ROOM (--on | --off)
  agent add --room ROOM --name NAME --provider claude|codex|cursor --pwd PATH
            [--args STRING] [--consent-hooks] [--orchestrates ROOM]
            [--system-prompt TEXT | --system-prompt-file PATH]
  agent orchestrate AGENT (--room ROOM | --none)
  agent read AGENT --source visible
  agent read AGENT [--source recent] --lines N
  agent permission AGENT
  agent approve-once AGENT --fingerprint FINGERPRINT --response allow-once
  agent focus AGENT
  agent rename AGENT NAME
  agent details AGENT (--on | --off)
  agent setup-confirm AGENT --confirm
  agent delete AGENT --confirm
  send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...] [--as AGENT]
  message status MESSAGE_ID
  request recover REQUEST_ID --confirm
  wait --message MESSAGE_ID [--timeout SECONDS]
  history --room ROOM
  settings color-blind (--on | --off)
  quit
  diagnostics

Every command accepts --request-id STRING and emits one JSON response.
ROOM and AGENT accept a name or numeric ID; ROOM also accepts master (any case) for the
MASTER room. Only MASTER agents orchestrate, each at most one work room: use
agent add --room master --orchestrates ROOM, or agent orchestrate to reassign or unassign.
A MASTER agent launches with an orchestrator system prompt, the built-in one unless
--system-prompt or --system-prompt-file replaces it; {{ROOM_NAME}} {{ROOM_ID}} {{AGENT_NAME}}
{{DOCS}} are filled in. Reassigning sends the orchestrator a message naming its new room.
--args \"--resume SESSION_ID\" (claude, cursor) or \"resume SESSION_ID\" (codex) adopts an
existing provider session by its UUID; quit that session elsewhere first.
Use --to all explicitly for all room agents.
send --as records the message as written by that room agent or the room's MASTER
orchestrator; --to all then skips it.
room seen clears a room's unread count without changing the visible Bus view.
room sound turns that room's new-message sound on or off; MASTER starts on, work rooms off.
state includes each agent's compactions and per-provider usage (5-hour and weekly used %).
Usage status \"unknown\" means Bus has no data yet, never that the allowance is unused.
wait polls every 200 ms, defaults to 60 seconds, and accepts 1–600 seconds.
message status, wait and history keep raw Markdown and list attached files as absolute paths.
focus queues a visible Bus view change; its receipt does not claim the view has rendered.
agent read also works while an agent is launching (e.g. to see a provider trust prompt);
runtime.session_verified is false until its provider session starts.
agent details and settings color-blind set the TUI toggles; state shows both.
quit queues the TUI's save-and-quit (as Ctrl+Q); its receipt only attests queuing.
Commands only connect to the existing instance in BUS_DATA_DIR; they never start or enable it.";

pub fn run(data_dir: &Path, args: &[String]) -> io::Result<()> {
    run_with(
        args,
        &mut io::stdout().lock(),
        |request, timeout| match timeout {
            Some(timeout) => control::request_with_timeout(data_dir, request, timeout),
            None => control::request(data_dir, request),
        },
    )
}

fn next_request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "bus-dev-{}-{}-{}",
        std::process::id(),
        super::io::now_ns(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
    )
}

fn run_with(
    args: &[String],
    output: &mut impl Write,
    send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
) -> io::Result<()> {
    let request_id = next_request_id();
    let response = match parse(args, &request_id) {
        Ok(command) => execute(command, send),
        Err(message) => Response::failure(&request_id, "invalid_arguments", message),
    };
    serde_json::to_writer(&mut *output, &response).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()?;
    if response.ok {
        Ok(())
    } else {
        Err(io::Error::other(
            response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "Developer command failed".into()),
        ))
    }
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
                .subcommand_required(true)
                .subcommand(toggle("color-blind")),
        )
        .subcommand(subcommand("diagnostics"))
        .subcommand(
            subcommand("room")
                .subcommand_required(true)
                .subcommand(subcommand("create").arg(value_arg("name").required(true)))
                .subcommand(subcommand("focus").arg(value_arg("room").required(true)))
                .subcommand(subcommand("seen").arg(value_arg("room").required(true)))
                .subcommand(toggle("sound").arg(value_arg("room").required(true)))
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
                    subcommand("orchestrate")
                        .arg(value_arg("agent").required(true))
                        .arg(value_arg("room").long("room").conflicts_with("none"))
                        .arg(flag("none")),
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
                .subcommand(subcommand("permission").arg(value_arg("agent").required(true)))
                .subcommand(
                    subcommand("approve-once")
                        .arg(value_arg("agent").required(true))
                        .arg(option("fingerprint"))
                        .arg(option("response").value_parser(["allow-once"])),
                )
                .subcommand(subcommand("focus").arg(value_arg("agent").required(true)))
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
                .arg(value_arg("as").long("as")),
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
    let (method, params) = match name {
        "state" => ("state", json!({})),
        "quit" => ("bus.quit", json!({})),
        "settings" => match args.subcommand() {
            Some(("color-blind", args)) => (
                "settings.color_blind",
                json!({"on": on_off(args, "settings color-blind")?}),
            ),
            _ => return Err("unknown settings command".into()),
        },
        "diagnostics" => ("diagnostics", json!({})),
        "room" => match args.subcommand() {
            Some(("focus", args)) => ("room.focus", json!({"room": required(args, "room")?})),
            Some(("seen", args)) => ("room.seen", json!({"room": required(args, "room")?})),
            Some(("sound", args)) => (
                "room.sound",
                json!({"room": required(args, "room")?, "on": on_off(args, "room sound")?}),
            ),
            Some(("create", args)) => ("room.create", json!({"name": required(args, "name")?})),
            Some(("rename", args)) => (
                "room.rename",
                json!({"room": required(args, "room")?, "name": required(args, "name")?}),
            ),
            Some(("notes", args)) => (
                "room.notes",
                json!({
                    "room": required(args, "room")?,
                    "text": args.get_one::<String>("text").map(String::as_str).unwrap_or(""),
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
            Some(("orchestrate", args)) => {
                let room = match (args.get_one::<String>("room"), args.get_flag("none")) {
                    (Some(room), false) => json!(room),
                    (None, true) => Value::Null,
                    _ => {
                        return Err(
                            "agent orchestrate needs exactly one of --room ROOM or --none".into(),
                        )
                    }
                };
                (
                    "agent.orchestrate",
                    json!({"agent": required(args, "agent")?, "room": room}),
                )
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
            Some(("permission", args)) => (
                "agent.permission.observe",
                json!({"agent": required(args, "agent")?}),
            ),
            Some(("approve-once", args)) => (
                "agent.permission.approve_once",
                json!({
                    "agent": required(args, "agent")?,
                    "fingerprint": required(args, "fingerprint")?,
                    "response": required(args, "response")?,
                }),
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
        ] {
            assert!(command(args).is_err(), "{args:?}");
        }
        assert!(HELP.contains("room sound ROOM (--on | --off)"));
    }

    #[test]
    fn agent_orchestrate_requires_exactly_one_target() {
        for args in [
            &["agent", "orchestrate", "claude-orch"][..],
            &[
                "agent",
                "orchestrate",
                "claude-orch",
                "--room",
                "pr-123",
                "--none",
            ][..],
        ] {
            assert!(command(args).is_err(), "{args:?}");
        }
        assert!(HELP.contains("agent orchestrate AGENT (--room ROOM | --none)"));
        assert!(HELP.contains("[--orchestrates ROOM]"));
        assert!(HELP.contains("[--system-prompt TEXT | --system-prompt-file PATH]"));
    }

    #[test]
    fn toggles_require_exactly_one_of_on_or_off() {
        for args in [
            &["agent", "details", "2"][..],
            &["settings", "color-blind"],
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
                &["agent", "orchestrate", "claude-orch", "--room", "pr-123"],
                "agent.orchestrate",
                json!({"agent": "claude-orch", "room": "pr-123"}),
            ),
            (
                &["agent", "orchestrate", "claude-orch", "--none"],
                "agent.orchestrate",
                json!({"agent": "claude-orch", "room": null}),
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
            (&["quit"], "bus.quit", json!({})),
            (
                &["settings", "color-blind", "--on"],
                "settings.color_blind",
                json!({"on": true}),
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
    fn agent_approve_once_cli_exposes_only_fingerprint_and_fixed_response() {
        let observe = command(&["agent", "permission", "Reviewer"]).unwrap();
        assert_eq!(observe.method, "agent.permission.observe");
        assert_eq!(observe.params, json!({"agent":"Reviewer"}));
        let approve = command(&[
            "agent",
            "approve-once",
            "Reviewer",
            "--fingerprint",
            "fp-abc",
            "--response",
            "allow-once",
        ])
        .unwrap();
        assert_eq!(approve.method, "agent.permission.approve_once");
        assert_eq!(
            approve.params,
            json!({
                "agent":"Reviewer","fingerprint":"fp-abc","response":"allow-once"
            })
        );
        assert!(command(&[
            "agent",
            "approve-once",
            "Reviewer",
            "--fingerprint",
            " ",
            "--response",
            "allow-once",
        ])
        .is_err());
        assert!(command(&[
            "agent",
            "approve-once",
            "Reviewer",
            "--fingerprint",
            "fp",
            "--response",
            "yes",
        ])
        .is_err());
        assert!(command(&["agent", "permission", "Reviewer", "--keys", "enter"]).is_err());
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
}
