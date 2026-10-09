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
    let queued = command(&["send", "--room", "r", "--to", "a", "--text", "t", "--queue"]).unwrap();
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

#[test]
fn send_async_recovers_from_transient_server_busy_after_queued_receipt() {
    let mut output = Vec::new();
    let mut polls = 0;
    let mut pauses = Vec::new();
    let outcome = run_with_pause(
        &[
            "send", "--room", "7", "--to", "a1", "--text", "go", "--async",
        ]
        .map(String::from),
        &mut output,
        &mut |request: &Request, _| {
            if request.method == "message.send" {
                return Ok(Response::success(
                    &request.id,
                    json!({"message_id": 19, "request_ids": [41], "stage": "queued"}),
                ));
            }
            assert_eq!(request.method, "message.status");
            assert_eq!(request.params, json!({"message": "19"}));
            polls += 1;
            Ok(match polls {
                1 => Response::success(&request.id, status(vec![recipient(1, "queued", false)])),
                2 => Response::failure("", "server_busy", "Development connection limit reached"),
                3 => Response::success(&request.id, status(vec![recipient(1, "delivered", true)])),
                _ => panic!("must stop following once the recipient's turn ended"),
            })
        },
        |pause| pauses.push(pause),
    );
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        outcome.is_ok(),
        "transient overload must not end a queued send: {lines:?}"
    );
    assert_eq!(polls, 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["result"]["stage"], "queued");
    assert_eq!(lines[1]["result"]["requests"][0]["turn_ended"], true);
    assert_eq!(lines[0]["id"], lines[1]["id"]);
    assert!(pauses.iter().all(|pause| !pause.is_zero()));
}

#[test]
fn wait_recovers_from_transient_server_busy_before_its_deadline() {
    let mut polls = 0;
    let completed = json!({"message_id": 19, "complete": true, "requests": [{
        "request_id": 41, "agent_id": 1, "stage": "completed", "turn_ended": true,
    }]});
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
            assert!(timeout.is_some_and(|remaining| remaining <= Duration::from_secs(2)));
            polls += 1;
            Ok(match polls {
                1 => Response::success(&request.id, status(vec![recipient(1, "queued", false)])),
                2 => Response::failure("", "server_busy", "Development connection limit reached"),
                3 => Response::success(&request.id, completed.clone()),
                _ => panic!("must stop waiting once the message completed"),
            })
        },
    );
    assert!(
        outcome.is_ok(),
        "transient overload must not end wait: {response}"
    );
    assert_eq!(polls, 3);
    assert_eq!(response["id"], "wait-19");
    assert_eq!(response["result"], completed);
}

#[test]
fn agent_read_recovers_from_transient_server_busy_with_the_same_request_id() {
    let mut calls = 0;
    let (outcome, response) = run_captured(
        &[
            "agent",
            "read",
            "a1",
            "--source",
            "visible",
            "--request-id",
            "read-a1",
        ],
        |request, _| {
            assert_eq!(request.id, "read-a1");
            assert_eq!(request.method, "agent.read");
            calls += 1;
            Ok(match calls {
                1 => Response::failure("", "server_busy", "Development connection limit reached"),
                2 => Response::success(&request.id, json!({"text": "Ready"})),
                _ => panic!("must stop retrying once the read succeeds"),
            })
        },
    );
    assert!(
        outcome.is_ok(),
        "a transient overload must not fail an observation: {response}"
    );
    assert_eq!(calls, 2);
    assert_eq!(response["id"], "read-a1");
    assert_eq!(response["result"]["text"], "Ready");
}

#[test]
fn agent_read_reports_server_busy_after_its_retry_budget() {
    let mut calls = 0;
    let (outcome, response) = run_captured(
        &[
            "agent",
            "read",
            "a1",
            "--source",
            "visible",
            "--request-id",
            "read-a1",
        ],
        |request, _| {
            assert_eq!(request.id, "read-a1");
            calls += 1;
            Ok(Response::failure(
                "",
                "server_busy",
                "Development connection limit reached",
            ))
        },
    );
    assert!(outcome.is_err());
    assert_eq!(calls, 6);
    assert_eq!(response["id"], "read-a1");
    assert_eq!(response["error"]["code"], "server_busy");
}
