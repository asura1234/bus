//! Developer command grammar and request shape.
use clap::{builder::NonEmptyStringValueParser, Arg, ArgAction, ArgMatches, Command};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug)]
pub(super) struct ParsedCommand {
    pub(super) method: &'static str,
    pub(super) params: Value,
    pub(super) request_id: String,
    pub(super) wait_timeout: Option<Duration>,
    /// `send --async`: after sending, follow the message until every
    /// recipient's turn ended.
    pub(super) follow: bool,
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

pub(super) fn parse(args: &[String], request_id: &str) -> Result<ParsedCommand, String> {
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
