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
  room delete ROOM --confirm
  room focus ROOM
  agent add --room ROOM --name NAME --provider claude|codex|cursor --pwd PATH
            [--args STRING] [--consent-hooks]
  agent read AGENT [--source visible]
  agent focus AGENT
  agent setup-confirm AGENT --confirm
  agent delete AGENT --confirm
  send --room ROOM --to AGENT,AGENT --text TEXT [--file PATH ...]
  message status MESSAGE_ID
  request recover REQUEST_ID --confirm
  wait --message MESSAGE_ID [--timeout SECONDS]
  history --room ROOM
  diagnostics

Every command accepts --request-id STRING and emits one JSON response.
ROOM and AGENT accept a name or numeric ID. Use --to all explicitly for all room agents.
wait polls every 200 ms, defaults to 60 seconds, and accepts 1–600 seconds.
focus queues a visible Bus view change; its receipt does not claim the view has rendered.
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
        .subcommand(subcommand("diagnostics"))
        .subcommand(
            subcommand("room")
                .subcommand_required(true)
                .subcommand(subcommand("create").arg(value_arg("name").required(true)))
                .subcommand(subcommand("focus").arg(value_arg("room").required(true)))
                .subcommand(
                    subcommand("rename")
                        .arg(value_arg("room").required(true))
                        .arg(value_arg("name").required(true)),
                )
                .subcommand(
                    subcommand("delete")
                        .arg(value_arg("room").required(true))
                        .arg(flag("confirm").required(true)),
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
                        .arg(flag("consent-hooks")),
                )
                .subcommand(
                    subcommand("read")
                        .arg(value_arg("agent").required(true))
                        .arg(Arg::new("source").long("source").value_parser(["visible"])),
                )
                .subcommand(subcommand("focus").arg(value_arg("agent").required(true)))
                .subcommand(
                    subcommand("setup-confirm")
                        .arg(value_arg("agent").required(true))
                        .arg(flag("confirm").required(true)),
                )
                .subcommand(
                    subcommand("delete")
                        .arg(value_arg("agent").required(true))
                        .arg(flag("confirm").required(true)),
                ),
        )
        .subcommand(
            subcommand("send")
                .arg(option("room"))
                .arg(option("to"))
                .arg(option("text"))
                .arg(value_arg("file").long("file").action(ArgAction::Append)),
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
                    .arg(flag("confirm").required(true)),
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
        "diagnostics" => ("diagnostics", json!({})),
        "room" => match args.subcommand() {
            Some(("focus", args)) => ("room.focus", json!({"room": required(args, "room")?})),
            Some(("create", args)) => ("room.create", json!({"name": required(args, "name")?})),
            Some(("rename", args)) => (
                "room.rename",
                json!({"room": required(args, "room")?, "name": required(args, "name")?}),
            ),
            Some(("delete", args)) => (
                "room.delete",
                json!({"room": required(args, "room")?, "confirm": args.get_flag("confirm")}),
            ),
            _ => return Err("unknown room command".into()),
        },
        "agent" => match args.subcommand() {
            Some(("focus", args)) => ("agent.focus", json!({"agent": required(args, "agent")?})),
            Some(("add", args)) => (
                "agent.add",
                json!({
                    "room": required(args, "room")?, "name": required(args, "name")?,
                    "provider": required(args, "provider")?, "cwd": required(args, "pwd")?,
                    "extra_args": args.get_one::<String>("args").map(String::as_str).unwrap_or(""),
                    "consent_project_hooks": args.get_flag("consent-hooks"),
                }),
            ),
            Some(("read", args)) => {
                let mut params = json!({"agent": required(args, "agent")?});
                if let Some(source) = args.get_one::<String>("source") {
                    params["source"] = json!(source);
                }
                ("agent.read", params)
            }
            Some(("setup-confirm", args)) => (
                "agent.setup-confirm",
                json!({"agent": required(args, "agent")?, "confirm": args.get_flag("confirm")}),
            ),
            Some(("delete", args)) => (
                "agent.delete",
                json!({"agent": required(args, "agent")?, "confirm": args.get_flag("confirm")}),
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
            (
                "message.send",
                json!({
                    "room": required(args, "room")?, "to": recipients,
                    "text": required(args, "text")?,
                    "files": args.get_many::<String>("file").map(|values| values.cloned().collect::<Vec<_>>()).unwrap_or_default(),
                }),
            )
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
                json!({"request": required(args, "request")?, "confirm": args.get_flag("confirm")}),
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
                &["room", "delete", "planning", "--confirm"],
                "room.delete",
                json!({"room": "planning", "confirm": true}),
            ),
            (
                &["agent", "read", "Claude Agent"],
                "agent.read",
                json!({"agent": "Claude Agent"}),
            ),
            (
                &["agent", "read", "Claude Agent", "--source", "visible"],
                "agent.read",
                json!({"agent": "Claude Agent", "source": "visible"}),
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
    fn wait_is_a_bounded_message_status_query() {
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
            &["room", "delete", "7"],
            &["agent", "delete", "9"],
            &["agent", "setup-confirm", "9"],
            &["agent", "read", "Claude Agent", "--source", "recent"],
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
    fn wait_polls_fresh_status_until_complete_and_emits_only_final_response() {
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
    fn wait_timeout_retains_last_status_for_diagnosis() {
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
