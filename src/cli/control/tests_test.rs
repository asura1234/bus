use super::*;
use serde_json::json;

fn command(args: &[&str]) -> Result<ParsedCommand, String> {
    parse(
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        "generated-request",
    )
}

#[test]
fn feishu_cli_toggle_accepts_only_on_off_and_no_destination() {
    for (flag, on) in [("--on", true), ("--off", false)] {
        let parsed = command(&["settings", "feishu-notifications", flag]).unwrap();
        assert_eq!(parsed.method, "settings.feishu_notifications");
        assert_eq!(parsed.params, json!({"on": on}));
    }
    for flag in ["--user-id", "--chat-id", "--recipient", "--group"] {
        assert!(command(&["settings", "feishu-notifications", "--on", flag, "other"]).is_err());
    }
    assert!(command(&["settings", "feishu-notifications"]).is_err());
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
    assert!(DEV_HELP.contains("room sound ROOM (--on | --off) [--sound NAME]"));
    assert!(DEV_HELP.contains("settings room-sound (--on | --off) [--sound NAME]"));
    assert!(DEV_HELP.contains("\n  sounds\n"));
}

/// One sample command line per control method, so the tier table, the parser
/// and both help texts are checked against each other method by method.
const SAMPLES: &[&[&str]] = &[
    &["state"],
    &["settings"],
    &["room", "create", "r"],
    &["room", "rename", "r", "s"],
    &["room", "notes", "r", "--text", "t"],
    &["room", "delete", "r", "--confirm"],
    &["history", "--room", "r"],
    &[
        "agent",
        "add",
        "--room",
        "r",
        "--name",
        "a",
        "--provider",
        "codex",
        "--pwd",
        "/w",
    ],
    &["agent", "read", "a", "--source", "visible"],
    &["agent", "dialog", "a"],
    &[
        "agent",
        "choose",
        "a",
        "--option",
        "1",
        "--fingerprint",
        "f",
    ],
    &["agent", "answer", "a", "--skip", "--fingerprint", "f"],
    &["agent", "clear", "a"],
    &["agent", "interrupt", "a"],
    &["agent", "rename", "a", "b"],
    &["agent", "setup-confirm", "a", "--confirm"],
    &["agent", "delete", "a", "--confirm"],
    &["send", "--room", "r", "--to", "a", "--text", "t"],
    &["message", "status", "1"],
    &["message", "seen", "1", "--as", "orch"],
    &["request", "recover", "1", "--confirm"],
    &["room", "focus", "r"],
    &["room", "seen", "r"],
    &["room", "sound", "r", "--on"],
    &["agent", "focus", "a"],
    &["agent", "details", "a", "--on"],
    &["settings", "color-blind", "--on"],
    &["settings", "feishu-notifications", "--on"],
    &["settings", "room-sound", "--on"],
    &["sounds"],
    &["quit"],
    &["diagnostics"],
    #[cfg(feature = "dev-tools")]
    &["ui", "hits"],
];

#[test]
fn every_tier_table_method_parses_from_its_command_and_sits_in_its_tiers_help() {
    use crate::messaging::coordinator::{dev_tier_mentions, Tier, METHODS};
    assert_eq!(SAMPLES.len(), METHODS.len());
    for spec in METHODS {
        let sample = SAMPLES
            .iter()
            .find(|args| command(args).is_ok_and(|c| c.method == spec.method))
            .unwrap_or_else(|| panic!("no sample parses into {}", spec.method));
        assert!(sample.join(" ").starts_with(spec.command), "{sample:?}");
        let line = format!("\n  {}", spec.command);
        match spec.tier {
            Tier::Agent => assert!(
                AGENT_HELP.contains(&line),
                "{} missing from bus --help",
                spec.command
            ),
            Tier::Dev => {
                assert!(
                    DEV_HELP.contains(&line),
                    "{} missing from bus --dev --help",
                    spec.command
                );
                assert!(
                    !AGENT_HELP.contains(&line),
                    "{} is a dev tool in bus --help",
                    spec.command
                );
            }
        }
    }
    assert_eq!(dev_tier_mentions(AGENT_HELP), Vec::<String>::new());
}

/// `DEV_HELP` is compiled in from docs/dev-tools.md, so every source closure
/// that builds Bus must carry that file.
#[test]
fn the_dev_help_source_ships_with_the_cargo_and_nix_sources() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| std::fs::read_to_string(repo.join(path)).unwrap();
    assert!(read("Cargo.toml").contains("\"docs/dev-tools.md\""));
    assert!(read("packaging/nix/package.nix").contains("../../docs/dev-tools.md"));
}

/// Release builds (`./run prod`, `just build`, Nix and so npm) leave the
/// `dev-tools` test tooling off; the dev builds that resume with `--dev`
/// (`./run dev`, the dev cutover) turn it on.
#[test]
fn release_builds_leave_dev_tools_off_and_dev_builds_turn_it_on() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| std::fs::read_to_string(repo.join(path)).unwrap();
    let manifest: toml::Value = toml::from_str(&read("Cargo.toml")).unwrap();
    let features = &manifest["features"];
    assert!(features.get("dev-tools").is_some());
    assert!(features.get("default").is_none_or(|default| {
        !default
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str() == Some("dev-tools"))
    }));
    assert!(!read("packaging/nix/package.nix").contains("buildFeatures"));
    assert!(!read("justfile").contains("dev-tools"));
    let run = read("run");
    assert!(run.contains("build_bus --release --bin bus)"), "prod");
    assert!(
        run.contains("build_bus --bin bus --features dev-tools)"),
        "dev"
    );
    let cutover = read("tools/cutover.sh");
    assert!(cutover.contains("cargo build --locked --bin bus --features dev-tools "));
}

#[test]
fn orchestrators_have_no_reassign_command() {
    assert!(command(&["agent", "orchestrate", "claude-orch", "--none"]).is_err());
    assert!(!AGENT_HELP.contains("agent orchestrate AGENT"));
    assert!(AGENT_HELP.contains("exactly one work room for its whole life"));
    assert!(AGENT_HELP.contains("[--orchestrates ROOM]"));
    assert!(AGENT_HELP.contains("[--system-prompt TEXT | --system-prompt-file PATH]"));
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
    for provider in ["claude", "codex", "cursor", "grok"] {
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
fn wait_timeout_errors_explain_the_supported_range() {
    for timeout in ["0", "601", "999999", "nope"] {
        let error = command(&["wait", "--message", "19", "--timeout", timeout]).unwrap_err();
        assert!(error.contains("1-600"), "{error}");
    }
}

#[test]
fn wait_async_polls_until_reply_or_dialog_without_a_deadline() {
    for last in [
        json!({"message_id":19,"complete":true,"requests":[recipient(1, "replied", true)]}),
        json!({"message_id":19,"complete":false,"waiting_on_dialog":[1],"requests":[recipient(1, "delivered", false)]}),
    ] {
        let mut reads = 0;
        let (outcome, response) = run_captured(
            &["wait", "--message", "19", "--async"],
            |request, timeout| {
                assert_eq!(request.method, "message.status");
                assert!(timeout.is_none());
                reads += 1;
                Ok(Response::success(
                    &request.id,
                    if reads == 1 {
                        json!({"message_id":19,"complete":false,"requests":[recipient(1, "delivered", false)]})
                    } else {
                        last.clone()
                    },
                ))
            },
        );
        assert_eq!(reads, 2);
        assert_eq!(response["result"], last);
        assert_eq!(outcome.is_ok(), last["complete"] == true);
    }
    assert!(command(&["wait", "--message", "19", "--async", "--timeout", "60"]).is_err());
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

/// A `message.status` result; like Bus, it is complete once every request
/// replied or was abandoned.
fn status(requests: Vec<Value>) -> Value {
    let complete = requests
        .iter()
        .all(|r| r["stage"] == "replied" || r["stage"] == "abandoned");
    json!({"message_id": 19, "complete": complete, "requests": requests})
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
fn send_async_emits_one_json_document() {
    let mut output = Vec::new();
    run_with_pause(
        &[
            "send", "--room", "7", "--to", "a1", "--text", "go", "--async",
        ]
        .map(String::from),
        &mut output,
        &mut |request, _| {
            Ok(Response::success(
                &request.id,
                if request.method == "message.send" {
                    json!({"message_id":19,"request_ids":[41],"stage":"queued"})
                } else {
                    status(vec![recipient(1, "replied", true)])
                },
            ))
        },
        |_| {},
    )
    .unwrap();
    let response: Value = serde_json::from_slice(&output).expect("one JSON document");
    assert_eq!(response["result"]["message_id"], 19);
    assert_eq!(response["result"]["requests"][0]["turn_ended"], true);
}

#[test]
fn send_async_returns_the_message_id_and_receipt_once_the_message_completed() {
    let (outcome, lines, timeouts, pauses) = run_async(
        &[],
        [
            status(vec![recipient(1, "queued", false)]),
            status(vec![recipient(1, "delivered", false)]),
            status(vec![recipient(1, "replied", true)]),
        ],
    );
    assert!(outcome.is_ok());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["result"]["message_id"], 19);
    assert_eq!(
        lines[0]["result"]["send_receipt"]["request_ids"],
        json!([41])
    );
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[0]["result"]["requests"][0]["turn_ended"], true);
    assert!(lines[0]["id"].is_string());
    assert!(timeouts.iter().all(Option::is_none), "no per-read deadline");
    assert_eq!(pauses, [Duration::from_millis(200); 2]);
}

#[test]
fn send_async_has_no_time_limit_and_keeps_waiting_while_blocked() {
    // About 33 minutes of polling, far past wait's 600-second cap.
    let polls = 10_000;
    let statuses = (0..polls)
        .map(|_| status(vec![recipient(1, "delivered", false)]))
        .chain([status(vec![recipient(1, "replied", true)])]);
    let (outcome, lines, _, pauses) = run_async(&[], statuses);
    assert!(outcome.is_ok());
    assert_eq!(lines.len(), 1);
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
                recipient(2, "replied", true),
            ]),
        ],
    );
    assert!(outcome.is_ok());
    assert_eq!(pauses.len(), 2);
    assert_eq!(lines[0]["result"]["requests"][1]["turn_ended"], true);
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
    assert_eq!(lines[0]["ok"], false);
    assert_eq!(lines[0]["error"]["code"], "message_abandoned");
    assert_eq!(lines[0]["result"]["requests"][1]["stage"], "abandoned");
}

#[test]
fn send_async_reports_a_provider_error_with_its_own_code_so_callers_can_retry() {
    let mut failed = recipient(2, "abandoned", false);
    failed["reason"] = json!("provider_error: Selected model is at capacity.");
    let (outcome, lines, _, _) = run_async(&[], [status(vec![failed])]);
    let error = outcome.unwrap_err().to_string();
    assert!(error.contains("Selected model is at capacity."), "{error}");
    assert_eq!(lines[0]["error"]["code"], "message_provider_error");
}

/// Live e2e: a follower saw the turn end with no reply and exited 0, 5 s
/// before Bus attached the provider error. A settling turn is not done yet.
#[test]
fn async_followers_wait_for_a_settling_turn_and_report_its_provider_error() {
    let mut settling = recipient(1, "delivered", true);
    settling["settling"] = json!(true);
    let mut failed = recipient(1, "abandoned", false);
    failed["reason"] = json!("provider_error: Selected model is at capacity.");
    let (outcome, lines, _, _) = run_async(
        &[],
        [status(vec![settling.clone()]), status(vec![failed.clone()])],
    );
    assert!(outcome.is_err(), "{lines:?}");
    assert_eq!(lines[0]["error"]["code"], "message_provider_error");

    let mut reads = vec![status(vec![settling]), status(vec![failed])].into_iter();
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            Ok(Response::success(&request.id, reads.next().unwrap()))
        });
    assert!(outcome.is_err(), "{response}");
    assert_eq!(response["error"]["code"], "message_provider_error");
}

/// #43: an ended turn with no captured reply and no provider error is done
/// for --async, also once settlement closes it as `reply_not_captured`.
#[test]
fn async_followers_finish_a_settled_turn_whose_reply_was_not_captured() {
    let mut settling = recipient(1, "delivered", true);
    settling["settling"] = json!(true);
    let mut closed = recipient(1, "abandoned", false);
    closed["reason"] = json!("reply_not_captured");
    let (outcome, lines, _, _) = run_async(
        &[],
        [status(vec![settling.clone()]), status(vec![closed.clone()])],
    );
    assert!(outcome.is_ok(), "{lines:?}");
    assert_eq!(lines[0]["result"]["requests"][0]["stage"], "abandoned");

    let mut reads = vec![status(vec![settling]), status(vec![closed])].into_iter();
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            Ok(Response::success(&request.id, reads.next().unwrap()))
        });
    assert!(outcome.is_ok(), "{response}");
}

/// The incomplete status a joined message shows while its group reply settles
/// (see `a_joined_message_reads_ended_but_incomplete_while_its_group_reply_settles`).
fn joined_ended_unsettled() -> Value {
    let mut joined = recipient(1, "joined", true);
    joined["settling"] = json!(false);
    joined["group"] = json!(5);
    status(vec![joined])
}

/// Live e2e (#121): 5 of 100 async followers exited 0 on a joined message whose
/// turn read ended while `complete` was still false. A follower exits 0 only once
/// the message is complete, and reports an interrupt that lands in that window.
#[test]
fn async_followers_keep_waiting_on_an_ended_but_incomplete_joined_message() {
    let mut replied = recipient(1, "replied", true);
    replied["group"] = json!(5);
    let mut done = status(vec![replied]);
    done["complete"] = json!(true);
    let mut interrupted = recipient(1, "abandoned", false);
    interrupted["reason"] = json!("interrupted");
    let mut stopped = status(vec![interrupted]);
    stopped["complete"] = json!(true);

    let (outcome, lines, _, _) = run_async(&[], [joined_ended_unsettled(), done.clone()]);
    assert!(outcome.is_ok(), "{lines:?}");
    assert_eq!(lines[0]["result"]["complete"], true);
    let (outcome, lines, _, _) = run_async(&[], [joined_ended_unsettled(), stopped.clone()]);
    assert!(outcome.is_err(), "{lines:?}");
    assert_eq!(lines[0]["error"]["code"], "message_interrupted");

    for last in [done, stopped] {
        let mut reads = vec![joined_ended_unsettled(), last.clone()].into_iter();
        let (_, response) = run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            Ok(Response::success(&request.id, reads.next().unwrap()))
        });
        assert_eq!(response["result"], last, "{response}");
        let mut reads = vec![joined_ended_unsettled(), last.clone()].into_iter();
        let followed = follow_message(
            "send-follow",
            &json!(19),
            &mut |request, _| Ok(Response::success(&request.id, reads.next().unwrap())),
            &mut |_| {},
        );
        assert_eq!(followed.result, last);
        assert_eq!(followed.ok, last["requests"][0]["stage"] == "replied");
    }
}

#[test]
fn waits_started_after_reply_not_captured_settlement_preserve_async_and_bounded_contracts() {
    let mut closed = recipient(1, "abandoned", false);
    closed["reason"] = json!("reply_not_captured");
    closed["settling"] = json!(false);
    let mut snapshot = status(vec![closed]);
    snapshot["complete"] = json!(true);
    for asynchronous in [true, false] {
        let args = if asynchronous {
            vec!["wait", "--message", "19", "--async"]
        } else {
            vec!["wait", "--message", "19", "--timeout", "60"]
        };
        let mut reads = 0;
        let (outcome, response) = run_captured(&args, |request, _| {
            reads += 1;
            assert_eq!(reads, 1, "settled request must finish on its first read");
            Ok(Response::success(&request.id, snapshot.clone()))
        });
        if asynchronous {
            assert!(outcome.is_ok(), "{response}");
        } else {
            assert_eq!(exit_code(&outcome), Some(1), "{response}");
            assert_eq!(response["error"]["code"], "message_abandoned");
        }
        assert_eq!(response["result"], snapshot);
    }
    let sent = follow_message(
        "send-follow",
        &json!(19),
        &mut |request, _| Ok(Response::success(&request.id, snapshot.clone())),
        &mut |_| panic!("settled turn must finish immediately"),
    );
    assert!(sent.ok);
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
    assert_eq!(lines[0]["error"]["code"], "message_stalled");
}

/// Settlement closes an ended turn without a captured reply as
/// `reply_not_captured`, which is done; one still open when it stalls is not.
#[test]
fn send_async_reports_a_finished_turn_that_stalled_without_settling() {
    let (outcome, lines, _, _) = run_async(
        &[],
        [status(vec![stalled_recipient(
            1,
            "delivered",
            "transcript_unmatched",
            true,
        )])],
    );
    assert_eq!(exit_code(&outcome), Some(STALLED_EXIT_CODE));
    assert_eq!(lines[0]["error"]["code"], "message_stalled");
}

#[test]
fn send_async_does_not_complete_an_uncertain_unbound_submission_on_idle() {
    let mut uncertain = recipient(1, "submitting", true);
    uncertain["uncertain_outcome"] = json!(true);
    uncertain["start_bound"] = json!(false);
    let (outcome, _, _, pauses) = run_async(
        &[],
        [
            status(vec![uncertain]),
            status(vec![recipient(1, "replied", true)]),
        ],
    );
    assert!(outcome.is_ok());
    assert_eq!(pauses, [Duration::from_millis(200)]);
}

#[test]
fn send_async_accepts_a_captured_group_reply_after_uncertain_steering() {
    let mut replied = recipient(1, "replied", true);
    replied["uncertain_outcome"] = json!(true);
    replied["start_bound"] = json!(false);
    replied["group"] = json!(40);
    replied["reply"] = json!({"text": "parser and tests written"});
    let mut completed = status(vec![replied]);
    completed["complete"] = json!(true);
    let (outcome, _, _, pauses) = run_async(
        &[],
        [completed, status(vec![recipient(1, "replied", true)])],
    );
    assert!(outcome.is_ok());
    assert!(
        pauses.is_empty(),
        "a captured final reply must finish async immediately"
    );
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
fn send_async_rejects_a_message_to_the_developer() {
    let parsed = command(&[
        "send", "--room", "master", "--to", "human", "--text", "x", "--async",
    ]);
    assert!(parsed.unwrap_err().contains("--async waits for agents"));
    assert!(
        command(&["send", "--room", "7", "--to", "a1", "--text", "x", "--async"])
            .unwrap()
            .follow
    );
    assert!(AGENT_HELP.contains("[--queue] [--async]"));
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
    assert!(AGENT_HELP.contains("[--as AGENT] [--queue]"));
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
                3 => Response::success(&request.id, status(vec![recipient(1, "replied", true)])),
                _ => panic!("must stop following once the message completed"),
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
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["result"]["send_receipt"]["stage"], "queued");
    assert_eq!(lines[0]["result"]["requests"][0]["turn_ended"], true);
    assert!(lines[0]["id"].is_string());
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

#[test]
fn wait_preserves_last_status_when_server_busy_persists_until_deadline() {
    let pending = status(vec![recipient(1, "queued", false)]);
    let started = std::time::Instant::now();
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
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "wait must retain its deadline"
            );
            assert!(timeout.is_some_and(|remaining| remaining <= Duration::from_secs(1)));
            calls += 1;
            Ok(if calls == 1 {
                Response::success(&request.id, pending.clone())
            } else {
                Response::failure("", "server_busy", "Development connection limit reached")
            })
        },
    );
    assert!(outcome.is_err());
    assert!(calls >= 2);
    assert_eq!(response["id"], "wait-19");
    assert_eq!(response["error"]["code"], "timeout");
    assert_eq!(response["result"], pending);
}

#[test]
fn async_following_caps_overload_backoff_and_recovers_after_extended_server_busy() {
    let mut calls = 0;
    let mut pauses = Vec::new();
    let completed = status(vec![recipient(1, "replied", true)]);
    let response = follow_message(
        "follow-19",
        &json!(19),
        &mut |request: &Request, timeout| {
            assert_eq!(request.method, "message.status");
            assert_eq!(request.params, json!({"message": "19"}));
            assert!(timeout.is_none());
            calls += 1;
            assert!(calls <= 9, "following must end after the turn completes");
            Ok(if calls <= 8 {
                Response::failure("", "server_busy", "Development connection limit reached")
            } else {
                Response::success(&request.id, completed.clone())
            })
        },
        &mut |pause| pauses.push(pause),
    );
    assert!(response.ok);
    assert_eq!(response.result, completed);
    assert_eq!(calls, 9);
    assert_eq!(pauses.len(), 8);
    assert_eq!(pauses[0], Duration::from_millis(100));
    assert!(pauses.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(pauses.iter().all(|pause| *pause <= Duration::from_secs(2)));
    assert!(pauses[5..]
        .iter()
        .all(|pause| *pause == Duration::from_secs(2)));
}

#[test]
fn async_following_retries_coordinator_overload_without_reporting_bus_down() {
    let mut calls = 0;
    let mut pauses = Vec::new();
    let completed = status(vec![recipient(1, "replied", true)]);
    let response = follow_message(
        "coordinator-overload",
        &json!(19),
        &mut |request, timeout| {
            assert_eq!(request.method, "message.status");
            assert!(timeout.is_none());
            calls += 1;
            assert!(calls <= 33);
            if calls == 33 {
                return Ok(Response::success(&request.id, completed.clone()));
            }
            if calls % 4 == 0 {
                let code = if calls % 8 == 0 {
                    "coordinator_timeout"
                } else {
                    "coordinator_busy"
                };
                return Ok(Response::failure(
                    &request.id,
                    code,
                    "live coordinator is overloaded",
                ));
            }
            Err("transient connection loss".into())
        },
        &mut |pause| pauses.push(pause),
    );
    assert!(
        response.ok,
        "overload must not settle a live message: {:?}",
        response.error
    );
    assert_eq!(response.result, completed);
    assert_eq!(calls, 33);
    assert_eq!(pauses.len(), 32);
    assert!(pauses.contains(&Duration::from_secs(2)));
    assert!(pauses.iter().all(|pause| *pause <= Duration::from_secs(2)));
}

#[test]
fn wait_async_retries_coordinator_overload_status_reads() {
    let mut calls = 0;
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            assert_eq!(request.method, "message.status");
            calls += 1;
            Ok(match calls {
                1 => Response::failure(&request.id, "coordinator_timeout", "still active"),
                2 => Response::failure(&request.id, "coordinator_busy", "still active"),
                _ => Response::success(&request.id, status(vec![recipient(1, "replied", true)])),
            })
        });
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(calls, 3);
}

#[test]
fn coordinator_timeout_never_replays_a_message_send() {
    let mut calls = 0;
    let (outcome, response) = run_captured(
        &[
            "send", "--room", "7", "--to", "a1", "--text", "go", "--async",
        ],
        |request, _| {
            calls += 1;
            assert_eq!(request.method, "message.send");
            Ok(Response::failure(
                &request.id,
                "coordinator_timeout",
                "operation may still complete",
            ))
        },
    );
    assert!(outcome.is_err());
    assert_eq!(response["error"]["code"], "coordinator_timeout");
    assert_eq!(calls, 1);
}

/// `bus --help` hides the dev tier, but clap's own errors for an incomplete
/// or mistyped agent-tier command should not advertise dev tools either.
#[test]
fn parse_errors_do_not_advertise_dev_tools() {
    for args in [
        &["room"][..],
        &["agent"][..],
        &["settings", "x"][..],
        &["room", "focs", "r"][..],
    ] {
        let error = command(args).unwrap_err();
        for dev in [
            "focus",
            "seen",
            "sound",
            "details",
            "color-blind",
            "room-sound",
            "diagnostics",
            "sounds",
            "quit",
        ] {
            assert!(
                !error.contains(dev),
                "{args:?} names dev tool {dev}: {error}"
            );
        }
    }
}

#[test]
fn send_async_failure_keeps_the_send_receipt_and_message_id() {
    let (outcome, lines, _, _) = run_async(&[], [status(vec![recipient(1, "abandoned", false)])]);
    assert!(outcome.is_err());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["error"]["code"], "message_abandoned");
    assert_eq!(lines[0]["result"]["message_id"], 19);
    assert_eq!(
        lines[0]["result"]["send_receipt"]["request_ids"],
        json!([41])
    );
}

#[test]
fn wait_async_keeps_polling_through_more_server_busy_replies_than_a_plain_command_retries() {
    let mut reads = 0;
    let (outcome, response) = run_captured(
        &["wait", "--message", "19", "--async"],
        |request, _| {
            reads += 1;
            Ok(if reads <= 6 {
                Response::failure(&request.id, "server_busy", "busy")
            } else {
                Response::success(
                    &request.id,
                    json!({"message_id":19,"complete":true,"requests":[recipient(1, "replied", true)]}),
                )
            })
        },
    );
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(reads, 7);
}

#[test]
fn wait_async_with_an_explicit_timeout_names_the_conflicting_options() {
    let error = command(&["wait", "--message", "19", "--async", "--timeout", "60"]).unwrap_err();
    assert!(
        error.contains("--async") && error.contains("--timeout"),
        "{error}"
    );
}

#[test]
fn wait_async_keeps_waiting_through_a_transient_transport_error() {
    let mut reads = 0;
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            reads += 1;
            if reads == 2 {
                return Err("No developer endpoint is available".into());
            }
            Ok(Response::success(
                &request.id,
                status(vec![recipient(
                    1,
                    if reads >= 3 { "replied" } else { "delivered" },
                    reads >= 3,
                )]),
            ))
        });
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(reads, 3);
}

#[test]
fn wait_async_exits_when_bus_stays_down() {
    let mut reads = 0;
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            reads += 1;
            if reads > 6 {
                return Ok(Response::failure(
                    &request.id,
                    "test_limit",
                    "retry loop did not exit",
                ));
            }
            Err("connection refused".into())
        });
    assert!(outcome.is_err());
    assert_eq!(response["error"]["code"], "bus_down");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Bus is down"));
    assert_eq!(reads, 4);
}

#[test]
fn send_async_exits_when_bus_stays_down() {
    let mut reads = 0;
    let response = follow_message(
        "send-down",
        &json!(19),
        &mut |request, _| {
            reads += 1;
            if reads > 6 {
                return Ok(Response::failure(
                    &request.id,
                    "test_limit",
                    "retry loop did not exit",
                ));
            }
            Err("connection refused".into())
        },
        &mut |_| {},
    );
    assert_eq!(response.error.unwrap().code, "bus_down");
    assert_eq!(reads, 4);
}

#[test]
fn wait_async_retries_connected_control_disconnects() {
    let mut reads = 0;
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            reads += 1;
            if reads <= 5 {
                return Err("control_io: response connection closed or failed".into());
            }
            Ok(Response::success(
                &request.id,
                status(vec![recipient(1, "replied", true)]),
            ))
        });
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(reads, 6);
}

#[test]
fn connected_control_disconnects_preserve_wait_deadlines_and_send_no_replay() {
    let mut reads = 0;
    let (outcome, response) = run_captured(
        &["wait", "--message", "19", "--timeout", "1"],
        |_, remaining| {
            assert!(remaining.is_some());
            reads += 1;
            Err("control_io: response connection closed or failed".into())
        },
    );
    assert!(outcome.is_err());
    assert_eq!(response["error"]["code"], "timeout");
    assert!(reads >= 2);

    let mut sends = 0;
    let (outcome, response) = run_captured(
        &[
            "send", "--room", "7", "--to", "a1", "--text", "go", "--async",
        ],
        |_, _| {
            sends += 1;
            Err("control_io: response connection closed or failed".into())
        },
    );
    assert!(outcome.is_err());
    assert_eq!(response["error"]["code"], "transport_error");
    assert_eq!(sends, 1);
}

#[test]
fn async_waiters_exit_after_a_connected_endpoint_disappears() {
    for args in [
        vec!["wait", "--message", "19", "--async"],
        vec![
            "send", "--room", "7", "--to", "a1", "--text", "go", "--async",
        ],
    ] {
        let mut reads = 0;
        let (outcome, response) = run_captured(&args, |request, _| {
            if request.method == "message.send" {
                return Ok(Response::success(&request.id, json!({"message_id":19})));
            }
            reads += 1;
            if reads == 1 {
                Err("control_io: response connection closed or failed".into())
            } else {
                Err("control_unavailable: connect: connection refused".into())
            }
        });
        assert!(outcome.is_err());
        assert_eq!(response["error"]["code"], "bus_down");
        assert_eq!(reads, 5, "{args:?}");
    }
}

/// A live control server at its connection limit keeps excess callers waiting
/// or answers server_busy while its coordinator is stalled; the follower must
/// keep following instead of reporting that Bus is down.
#[cfg(unix)]
#[test]
fn async_following_survives_live_control_server_at_its_connection_limit() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!(
        "bcl-{:x}-{:x}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let _server = crate::messaging::control::start(&dir, tx).unwrap();
    let socket = dir.join("wait.sock");
    let mut stalled = Vec::new();
    let mut streams = Vec::new();
    for id in 0..16 {
        let mut stream = crate::platform::ipc::connect_local_stream(&socket).unwrap();
        let request = Request {
            id: format!("stalled-{id}"),
            method: "message.status".into(),
            params: json!({"message": "19"}),
        };
        writeln!(stream, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        stalled.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        streams.push(stream);
    }
    // Another overloaded caller holds the single server_busy slot.
    let busy_holder = crate::platform::ipc::connect_local_stream(&socket).unwrap();
    thread::sleep(Duration::from_millis(100));
    let completed = status(vec![recipient(1, "replied", true)]);
    let mut calls = 0;
    let mut transport_errors = Vec::new();
    let response = follow_message(
        "live-overload",
        &json!(19),
        &mut |request, _| {
            calls += 1;
            if calls > 6 {
                return Ok(Response::success(&request.id, completed.clone()));
            }
            let outcome = crate::messaging::control::request(&dir, request);
            if let Err(error) = &outcome {
                transport_errors.push(error.clone());
            }
            outcome
        },
        &mut |_| {},
    );
    drop(busy_holder);
    drop(streams);
    drop(stalled);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        response.ok,
        "live control server reported as {:?} after transport errors {transport_errors:?}",
        response.error
    );
}

#[test]
fn async_transport_budget_resets_after_successful_status_reads() {
    let mut reads = 0;
    let mut pauses = 0;
    let response = follow_message(
        "reset",
        &json!(19),
        &mut |request, _| {
            reads += 1;
            if reads % 4 != 0 {
                return Err("temporary loss".into());
            }
            Ok(Response::success(
                &request.id,
                status(vec![recipient(
                    1,
                    if reads == 12 { "replied" } else { "delivered" },
                    reads == 12,
                )]),
            ))
        },
        &mut |_| {
            pauses += 1;
        },
    );
    assert!(response.ok);
    assert_eq!(reads, 12);
    assert_eq!(pauses, 11);

    let mut reads = 0;
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            reads += 1;
            if reads % 4 != 0 {
                return Err("temporary loss".into());
            }
            Ok(Response::success(
                &request.id,
                status(vec![recipient(
                    1,
                    if reads == 8 { "replied" } else { "delivered" },
                    reads == 8,
                )]),
            ))
        });
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(reads, 8);
}

#[test]
fn wait_timeout_negative_or_blank_values_explain_the_supported_range() {
    for timeout in ["-1", ""] {
        let error = command(&["wait", "--message", "19", "--timeout", timeout]).unwrap_err();
        assert!(error.contains("1-600"), "{timeout:?}: {error}");
    }
}

#[test]
fn wait_async_recovers_before_first_status_and_still_stops_at_a_dialog() {
    let mut reads = 0;
    let (outcome, response) = run_captured(
        &["wait", "--message", "19", "--async"],
        |request, timeout| {
            assert!(timeout.is_none());
            reads += 1;
            if reads <= 2 {
                return Err("temporary connection loss".into());
            }
            Ok(Response::success(
                &request.id,
                json!({"message_id":19,"complete":false,"waiting_on_dialog":[1],"requests":[recipient(1, "delivered", false)]}),
            ))
        },
    );
    assert_eq!(reads, 3);
    assert!(outcome.is_err());
    assert_eq!(response["error"]["code"], "agent_waiting_on_dialog");
    assert_eq!(response["result"]["waiting_on_dialog"], json!([1]));
}

#[test]
fn bounded_wait_still_reports_transport_error_without_retrying_it() {
    let mut reads = 0;
    let pending =
        json!({"message_id":19,"complete":false,"requests":[recipient(1, "delivered", false)]});
    let (outcome, response) = run_captured(
        &["wait", "--message", "19", "--timeout", "60"],
        |request, timeout| {
            assert!(timeout.is_some());
            reads += 1;
            if reads == 2 {
                return Err("connection lost".into());
            }
            Ok(Response::success(&request.id, pending.clone()))
        },
    );
    assert!(outcome.is_err());
    assert_eq!(reads, 2);
    assert_eq!(response["error"]["code"], "transport_error");
    assert_eq!(response["result"], pending);
}

#[test]
fn wait_timeout_attached_and_blank_values_use_the_wait_diagnostics() {
    for args in [
        vec!["wait", "--message", "19", "--timeout", ""],
        vec!["wait", "--message", "19", "--timeout=-1"],
    ] {
        let error = command(&args).unwrap_err();
        assert!(error.contains("1-600"), "{error}");
    }
    let error = command(&["wait", "--message", "19", "--timeout=60", "--async"]).unwrap_err();
    assert!(
        error.contains("--async") && error.contains("--timeout"),
        "{error}"
    );
}

fn restart_statuses() -> Value {
    serde_json::from_str(include_str!("tests/fixtures/restart-statuses.json")).unwrap()
}

/// #43's snapshots, recorded across a Bus restart: turns that ended with no
/// captured reply are still open, so async waits keep following them until
/// settlement closes them as `reply_not_captured`.
#[test]
fn async_waits_follow_recorded_ended_turns_until_they_settle() {
    for snapshot in restart_statuses()["ended"].as_array().unwrap() {
        assert_eq!(snapshot["complete"], false);
        let mut settled = snapshot.clone();
        settled["complete"] = json!(true);
        settled["requests"][0]["stage"] = json!("abandoned");
        settled["requests"][0]["reason"] = json!("reply_not_captured");
        let message = snapshot["message_id"].to_string();
        let mut reads = vec![snapshot.clone(), settled.clone()].into_iter();
        let (outcome, response) = run_captured(
            &["wait", "--message", &message, "--async"],
            |request, timeout| {
                assert_eq!(request.method, "message.status");
                assert!(timeout.is_none());
                Ok(Response::success(&request.id, reads.next().unwrap()))
            },
        );
        assert!(outcome.is_ok(), "{response}");
        assert_eq!(reads.len(), 0, "wait --async finished on an open request");
        assert_eq!(response["result"], settled);
        let mut reads = vec![snapshot.clone(), settled.clone()].into_iter();
        let sent = follow_message(
            "send-follow",
            &snapshot["message_id"],
            &mut |request, _| Ok(Response::success(&request.id, reads.next().unwrap())),
            &mut |_| {},
        );
        assert!(sent.ok);
        assert_eq!(response["result"], sent.result);
    }
}

#[test]
fn wait_async_reports_recorded_abandonment_with_the_send_async_exit_code() {
    let snapshot = restart_statuses()["abandoned"].clone();
    let message = snapshot["message_id"].to_string();
    let (outcome, response) =
        run_captured(&["wait", "--message", &message, "--async"], |request, _| {
            Ok(Response::success(&request.id, snapshot.clone()))
        });
    assert_eq!(exit_code(&outcome), Some(1), "{response}");
    assert_eq!(response["error"]["code"], "message_abandoned");
    assert_eq!(response["result"], snapshot);
    let sent = follow_message(
        "send-follow",
        &snapshot["message_id"],
        &mut |request, _| Ok(Response::success(&request.id, snapshot.clone())),
        &mut |_| panic!("abandonment is terminal"),
    );
    assert!(!sent.ok);
    assert_eq!(response["error"]["code"], sent.error.unwrap().code);
    assert_eq!(response["result"], sent.result);
}

#[test]
fn bounded_wait_reports_recorded_abandonment_as_failure() {
    let snapshot = restart_statuses()["abandoned"].clone();
    let message = snapshot["message_id"].to_string();
    let (outcome, response) = run_captured(
        &["wait", "--message", &message, "--timeout", "60"],
        |request, _| Ok(Response::success(&request.id, snapshot.clone())),
    );
    assert_eq!(exit_code(&outcome), Some(1), "{response}");
    assert_eq!(response["error"]["code"], "message_abandoned");
    assert_eq!(response["result"], snapshot);
}

#[test]
fn wait_async_and_send_async_agree_on_invalidated_joined_stalls() {
    let mut snapshot = restart_statuses()["joined"].clone();
    snapshot["requests"][0]["stage"] = json!("stalled");
    snapshot["requests"][0]["stalled_from"] = json!("joined");
    snapshot["requests"][0]["reason"] = json!("session_invalidated");
    let message = snapshot["message_id"].to_string();
    let (outcome, response) =
        run_captured(&["wait", "--message", &message, "--async"], |request, _| {
            Ok(Response::success(&request.id, snapshot.clone()))
        });
    assert_eq!(exit_code(&outcome), Some(3));
    let sent = follow_message(
        "send-follow",
        &snapshot["message_id"],
        &mut |request, _| Ok(Response::success(&request.id, snapshot.clone())),
        &mut |_| panic!("invalidated stall is terminal"),
    );
    assert!(!sent.ok);
    assert_eq!(response["error"]["code"], sent.error.unwrap().code);
    assert_eq!(response["result"], sent.result);
}

/// A turn that ended yet stalled was never settled, so it is not done; the
/// wait reports the stall even while another recipient still works.
#[test]
fn wait_async_reports_a_finished_stall_while_other_recipients_work() {
    let mut finished = recipient(1, "stalled", true);
    finished["stalled_from"] = json!("delivered");
    finished["reason"] = json!("transcript_unmatched");
    let mut reads = 0;
    let (outcome, response) =
        run_captured(&["wait", "--message", "19", "--async"], |request, _| {
            reads += 1;
            Ok(Response::success(
                &request.id,
                status(vec![finished.clone(), recipient(2, "delivered", false)]),
            ))
        });
    assert_eq!(exit_code(&outcome), Some(STALLED_EXIT_CODE), "{response}");
    assert_eq!(response["error"]["code"], "message_stalled");
    assert_eq!(reads, 1);
}

#[test]
fn async_wait_and_send_share_settlement_across_recipient_stages() {
    for stage in [
        "queued",
        "submitting",
        "awaiting_start",
        "delivered",
        "joined",
        "replied",
        "abandoned",
        "stalled",
    ] {
        for ended in [false, true] {
            let snapshot = status(vec![recipient(1, stage, ended)]);
            let final_status = status(vec![recipient(1, "replied", true)]);
            let mut wait_reads = 0;
            let (outcome, waited) =
                run_captured(&["wait", "--message", "19", "--async"], |request, _| {
                    wait_reads += 1;
                    assert!(wait_reads <= 2);
                    Ok(Response::success(
                        &request.id,
                        if wait_reads == 1 {
                            snapshot.clone()
                        } else {
                            final_status.clone()
                        },
                    ))
                });
            let mut send_reads = 0;
            let sent = follow_message(
                "follow",
                &json!(19),
                &mut |request, _| {
                    send_reads += 1;
                    assert!(send_reads <= 2);
                    Ok(Response::success(
                        &request.id,
                        if send_reads == 1 {
                            snapshot.clone()
                        } else {
                            final_status.clone()
                        },
                    ))
                },
                &mut |_| {},
            );
            assert_eq!(wait_reads, send_reads, "{stage} ended={ended}");
            assert_eq!(outcome.is_ok(), sent.ok, "{stage} ended={ended}");
            assert_eq!(waited["result"], sent.result);
            assert_eq!(
                waited["error"]["code"].as_str(),
                sent.error.as_ref().map(|error| error.code.as_str())
            );
        }
    }
}

/// revB-claude round 5 probe: a bounded wait on an abandoned request exits 1
/// on master; e2e saw exit 0 for provider_error and reply_not_captured.
#[test]
fn rev_b_claude_r5_bounded_wait_fails_on_any_abandoned_request() {
    for (reason, code) in [
        (
            "provider_error: Selected model is at capacity.",
            "message_provider_error",
        ),
        ("reply_not_captured", "message_abandoned"),
    ] {
        let mut closed = recipient(1, "abandoned", false);
        closed["reason"] = json!(reason);
        let mut snapshot = status(vec![closed]);
        snapshot["complete"] = json!(true);
        let (outcome, response) = run_captured(
            &["wait", "--message", "19", "--timeout", "5"],
            |request, _| Ok(Response::success(&request.id, snapshot.clone())),
        );
        assert_eq!(exit_code(&outcome), Some(1), "{reason}: {response}");
        assert_eq!(response["error"]["code"], code, "{reason}: {response}");
    }
}

#[test]
fn agent_interrupt_names_one_agent_or_one_room_with_optional_text_and_author() {
    let parsed = command(&["agent", "interrupt", "worker", "--as", "orch"]).unwrap();
    assert_eq!(parsed.method, "agent.interrupt");
    assert_eq!(parsed.params, json!({"agent": "worker", "as": "orch"}));
    let parsed = command(&["agent", "interrupt", "--room", "lab", "--text", "pause"]).unwrap();
    assert_eq!(parsed.params, json!({"room": "lab", "text": "pause"}));
    for args in [
        &["agent", "interrupt"][..],
        &["agent", "interrupt", "worker", "--room", "lab"][..],
        &["agent", "interrupt", "worker", "--text", " "][..],
    ] {
        assert!(command(args).is_err(), "must reject {args:?}");
    }
}

/// One agent row of a `state` result.
fn agent_row(id: u64, status: &str, pending: bool) -> Value {
    let mut row = json!({"id": id, "name": format!("a{id}"), "room_id": 7, "status": status});
    if pending {
        row["interrupt_pending"] = json!(true);
    }
    row
}

/// Runs `agent interrupt` against a scripted `agent.interrupt` receipt and
/// scripted `state` reads; returns the outcome, the one printed response,
/// every request sent, and the pauses between state reads.
fn run_interrupt(
    args: &[&str],
    receipt: Value,
    states: impl IntoIterator<Item = Vec<Value>>,
) -> (io::Result<()>, Value, Vec<Request>, Vec<Duration>) {
    run_interrupt_reads(
        args,
        receipt,
        states
            .into_iter()
            .map(|agents| Ok(Response::success("state", json!({"agents": agents})))),
    )
}

fn run_interrupt_reads(
    args: &[&str],
    receipt: Value,
    states: impl IntoIterator<Item = Result<Response, String>>,
) -> (io::Result<()>, Value, Vec<Request>, Vec<Duration>) {
    let mut states = states.into_iter();
    let mut requests = Vec::new();
    let mut pauses = Vec::new();
    let mut output = Vec::new();
    let outcome = run_with_pause(
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        &mut output,
        &mut |request: &Request, _| {
            requests.push(request.clone());
            if request.method == "state" {
                return states.next().expect("read past the last state");
            }
            Ok(Response::success(
                &request.id,
                match request.method.as_str() {
                    "agent.interrupt" => receipt.clone(),
                    "message.send" => {
                        json!({"message_id": 30, "request_ids": [31], "stage": "queued"})
                    }
                    other => panic!("unexpected {other}"),
                },
            ))
        },
        |pause| pauses.push(pause),
    );
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output.lines().count(), 1, "one JSON response: {output:?}");
    (
        outcome,
        serde_json::from_str(&output).unwrap(),
        requests,
        pauses,
    )
}

fn interrupting(agent: u64) -> Value {
    json!({"room_id": 7, "agents": [{"agent_id": agent, "name": format!("a{agent}"), "outcome": "interrupting", "key": "esc"}]})
}

#[test]
fn agent_interrupt_reports_stopped_once_bus_saw_the_turn_end() {
    let (outcome, response, requests, pauses) = run_interrupt(
        &["agent", "interrupt", "a4"],
        interrupting(4),
        [
            vec![agent_row(4, "working", true)],
            vec![agent_row(4, "idle", true)],
            vec![agent_row(4, "idle", false)],
        ],
    );
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(
        response["result"],
        json!({"agent_id": 4, "name": "a4", "room_id": 7, "outcome": "stopped", "key": "esc"})
    );
    let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(methods, ["agent.interrupt", "state", "state", "state"]);
    assert_eq!(pauses, [Duration::from_millis(200); 2]);
}

#[test]
fn agent_interrupt_text_is_sent_as_a_new_message_after_the_stop() {
    let (outcome, response, requests, _) = run_interrupt(
        &[
            "agent",
            "interrupt",
            "a4",
            "--text",
            "pause; reply with status",
            "--as",
            "orch",
        ],
        interrupting(4),
        [vec![agent_row(4, "idle", false)]],
    );
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(response["result"]["outcome"], "stopped");
    assert_eq!(response["result"]["message_id"], 30);
    let send = requests.last().unwrap();
    assert_eq!(send.method, "message.send");
    assert_eq!(
        send.params,
        json!({"room": "7", "to": ["4"], "text": "pause; reply with status", "files": [], "as": "orch"})
    );
    // The interrupt itself carries only the agent and its author.
    assert_eq!(requests[0].params, json!({"agent": "a4", "as": "orch"}));
}

#[test]
fn agent_interrupt_retries_failed_state_reads_before_delivering_text() {
    let mut failures = vec![Err("socket read timed out".into())];
    for code in ["server_busy", "coordinator_busy", "coordinator_timeout"] {
        failures.push(Ok(Response::failure("state", code, "try again")));
    }
    let mut failed_snapshot = Response::failure("state", "coordinator_busy", "try again");
    failed_snapshot.result = json!({"agents": []});
    failures.push(Ok(failed_snapshot));
    for result in [Value::Null, json!({}), json!({"agents": {}})] {
        failures.push(Ok(Response::success("state", result)));
    }
    for failure in failures {
        let label = format!("{failure:?}");
        let (outcome, response, requests, pauses) = run_interrupt_reads(
            &[
                "agent",
                "interrupt",
                "a4",
                "--as",
                "orch",
                "--text",
                "report status",
            ],
            interrupting(4),
            [
                failure,
                Ok(Response::success(
                    "state",
                    json!({"agents": [agent_row(4, "idle", true)]}),
                )),
                Ok(Response::success(
                    "state",
                    json!({"agents": [agent_row(4, "idle", false)]}),
                )),
            ],
        );
        assert!(outcome.is_ok(), "{label}: {response}");
        assert_eq!(response["result"]["outcome"], "stopped", "{label}");
        assert_eq!(response["result"]["message_id"], 30);
        let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
        assert_eq!(
            methods,
            ["agent.interrupt", "state", "state", "state", "message.send"]
        );
        assert_eq!(
            requests.last().unwrap().params,
            json!({"room": "7", "to": ["4"], "text": "report status", "files": [], "as": "orch"})
        );
        assert_eq!(pauses, [Duration::from_millis(200); 2]);
    }
}

#[test]
fn agent_interrupt_reports_unconfirmed_stop_when_state_reads_keep_failing() {
    let (outcome, response, requests, pauses) = run_interrupt_reads(
        &["agent", "interrupt", "a4", "--text", "report status"],
        interrupting(4),
        std::iter::repeat_with(|| {
            Ok(Response::failure(
                "state",
                "server_busy",
                "Control connection limit reached",
            ))
        }),
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["error"]["code"], "interrupt_failed");
    let reason = response["result"]["reason"].as_str().unwrap();
    assert!(reason.contains("could not confirm"), "{response}");
    assert!(reason.contains("server_busy"), "{response}");
    assert!(!reason.contains("deleted"), "{response}");
    assert!(!reason.contains("still working"), "{response}");
    assert!(requests.iter().all(|r| r.method != "message.send"));
    assert!(pauses.iter().sum::<Duration>() >= Duration::from_secs(15));
    assert!(
        requests.len() <= 77,
        "failed reads must respect the deadline"
    );
}

#[test]
fn agent_interrupt_reports_deleted_only_from_a_valid_agent_list() {
    let (outcome, response, requests, _) = run_interrupt(
        &["agent", "interrupt", "a4", "--text", "report status"],
        interrupting(4),
        [vec![agent_row(5, "idle", false)]],
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["result"]["reason"], "agent a4 was deleted");
    assert!(requests.iter().all(|r| r.method != "message.send"));
}

#[test]
fn agent_interrupt_still_reports_a_deletion_seen_after_a_busy_read() {
    let (outcome, response, requests, _) = run_interrupt_reads(
        &["agent", "interrupt", "a4", "--text", "report status"],
        interrupting(4),
        [
            Ok(Response::failure("state", "server_busy", "try again")),
            Ok(Response::success(
                "state",
                json!({"agents": [agent_row(5, "idle", false)]}),
            )),
        ],
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["result"]["reason"], "agent a4 was deleted");
    assert!(requests.iter().all(|r| r.method != "message.send"));
}

#[test]
fn agent_interrupt_of_an_idle_agent_still_delivers_the_text() {
    let receipt =
        json!({"room_id": 7, "agents": [{"agent_id": 4, "name": "a4", "outcome": "already_idle"}]});
    let (outcome, response, requests, _) = run_interrupt(
        &["agent", "interrupt", "a4", "--text", "next task"],
        receipt,
        [],
    );
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(response["result"]["outcome"], "already_idle");
    assert_eq!(response["result"]["message_id"], 30);
    let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(methods, ["agent.interrupt", "message.send"]);
}

#[test]
fn agent_interrupt_fails_with_a_reason_when_the_turn_does_not_stop() {
    let (outcome, response, requests, pauses) = run_interrupt(
        &["agent", "interrupt", "a4", "--text", "pause"],
        interrupting(4),
        std::iter::repeat_with(|| vec![agent_row(4, "working", true)]),
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "interrupt_failed");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("still working"),
        "{response}"
    );
    assert_eq!(response["result"]["outcome"], "failed");
    assert!(requests.iter().all(|r| r.method != "message.send"));
    assert!(pauses.iter().sum::<Duration>() >= Duration::from_secs(15));

    let (outcome, response, _, _) = run_interrupt(
        &["agent", "interrupt", "a4"],
        interrupting(4),
        [vec![agent_row(4, "unavailable", true)]],
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unavailable"),
        "{response}"
    );

    let refused = json!({"room_id": 7, "agents": [{"agent_id": 4, "name": "a4", "outcome": "failed", "reason": "agent a4 is unavailable"}]});
    let (outcome, response, requests, _) = run_interrupt(
        &["agent", "interrupt", "a4", "--text", "pause"],
        refused,
        [],
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["error"]["code"], "interrupt_failed");
    assert_eq!(requests.len(), 1);
}

#[test]
fn a_room_interrupt_reports_every_agent_and_texts_the_stopped_ones() {
    let receipt = json!({"room_id": 7, "agents": [
        {"agent_id": 4, "name": "a4", "outcome": "interrupting", "key": "esc"},
        {"agent_id": 5, "name": "a5", "outcome": "interrupting", "key": "ctrl+c"},
    ]});
    let (outcome, response, requests, _) = run_interrupt(
        &["agent", "interrupt", "--room", "lab", "--text", "pause"],
        receipt,
        [
            vec![agent_row(4, "idle", false), agent_row(5, "working", true)],
            vec![agent_row(4, "idle", false), agent_row(5, "idle", false)],
        ],
    );
    assert!(outcome.is_ok(), "{response}");
    assert_eq!(response["result"]["agents"][0]["outcome"], "stopped");
    assert_eq!(response["result"]["agents"][1]["outcome"], "stopped");
    assert_eq!(response["result"]["message_id"], 30);
    assert_eq!(requests.last().unwrap().params["to"], json!(["4", "5"]));
}

#[test]
fn waits_end_with_message_interrupted_when_the_recipient_was_interrupted() {
    let mut interrupted = recipient(1, "abandoned", false);
    interrupted["reason"] = json!("interrupted");
    let (outcome, lines, _, _) = run_async(&[], [status(vec![interrupted.clone()])]);
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(lines[0]["error"]["code"], "message_interrupted");
    assert!(
        lines[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("interrupted"),
        "{lines:?}"
    );
    for args in [
        &["wait", "--message", "19", "--async"][..],
        &["wait", "--message", "19", "--timeout", "5"][..],
    ] {
        let read = status(vec![interrupted.clone()]);
        let (outcome, response) = run_captured(args, |request, _| {
            Ok(Response::success(&request.id, read.clone()))
        });
        assert_eq!(exit_code(&outcome), Some(1), "{args:?}");
        assert_eq!(response["error"]["code"], "message_interrupted", "{args:?}");
    }
}

#[test]
fn a_room_interrupt_delivers_follow_up_to_stopped_agents_despite_another_failure() {
    let receipt = json!({"room_id": 7, "agents": [
        {"agent_id": 4, "name": "a4", "outcome": "interrupting", "key": "esc"},
        {"agent_id": 5, "name": "a5", "outcome": "failed", "reason": "The agent has no terminal yet"},
    ]});
    let (outcome, response, requests, _) = run_interrupt(
        &[
            "agent",
            "interrupt",
            "--room",
            "lab",
            "--text",
            "report status",
        ],
        receipt,
        [vec![agent_row(4, "idle", false)]],
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["result"]["agents"][0]["outcome"], "stopped");
    let sends: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "message.send")
        .collect();
    assert_eq!(
        sends.len(),
        1,
        "the stopped worker must receive its follow-up despite another worker failing: {response}"
    );
    assert_eq!(sends[0].params["to"], json!(["4"]));
    assert_eq!(sends[0].params["text"], "report status");
}

#[test]
fn interrupt_state_reads_are_bounded_by_the_stop_deadline() {
    let mut output = Vec::new();
    let outcome = run_with_pause(
        &["agent", "interrupt", "a4"].map(String::from),
        &mut output,
        &mut |request: &Request, timeout| match request.method.as_str() {
            "agent.interrupt" => Ok(Response::success(&request.id, interrupting(4))),
            "state" => {
                assert!(
                    timeout.is_some_and(|duration| duration <= Duration::from_secs(15)),
                    "a status read must not outlive the advertised stop deadline: {timeout:?}"
                );
                Ok(Response::success(
                    &request.id,
                    json!({"agents": [agent_row(4, "idle", false)]}),
                ))
            }
            other => panic!("unexpected {other}"),
        },
        |_| {},
    );
    assert!(outcome.is_ok());
}
#[test]
fn feishu_cli_observes_pending_status_and_reports_notifications_off() {
    let mut output = Vec::new();
    let mut methods = Vec::new();
    let result = run_with_pause(
        &[
            "settings".into(),
            "feishu-notifications".into(),
            "--on".into(),
        ],
        &mut output,
        &mut |request: &Request, _| {
            methods.push(request.method.clone());
            Ok(Response::success(
                &request.id,
                if request.method == "settings.feishu_notifications" {
                    json!({"feishu":{"enabled":false,"pending":true}})
                } else {
                    json!({"settings":{"feishu":{"enabled":false,"pending":false,"error":"notifications remain off"}}})
                },
            ))
        },
        |_| {},
    );
    assert!(
        result.is_err(),
        "CLI must report that notifications remain off"
    );
    assert_eq!(methods, ["settings.feishu_notifications", "settings"]);
}

#[test]
fn feishu_enable_status_failure_preserves_caller_request_id() {
    let mut output = Vec::new();
    let result = run_with_pause(
        &[
            "settings".into(),
            "feishu-notifications".into(),
            "--on".into(),
            "--request-id".into(),
            "enable-notifications".into(),
        ],
        &mut output,
        &mut |request: &Request, _| {
            Ok(if request.method == "settings.feishu_notifications" {
                Response::success(
                    &request.id,
                    json!({"feishu":{"enabled":false,"pending":true}}),
                )
            } else {
                Response::failure(
                    &request.id,
                    "coordinator_busy",
                    "notification status unavailable",
                )
            })
        },
        |_| {},
    );

    assert!(result.is_err());
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["error"]["code"], "coordinator_busy");
    assert_eq!(response["id"], "enable-notifications");
}

/// Sharing the interactive socket with slow status reads rejects mutations once
/// followers fill it, even though every follower reconnects between polls.
#[cfg(unix)]
#[test]
fn one_hundred_async_waiters_do_not_starve_interrupt_or_new_send() {
    use crate::messaging::{control, coordinator::BusCommand};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Instant;
    let dir = std::env::temp_dir().join(format!(
        "bcl100-{:x}-{:x}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
    let (tx, rx) = std::sync::mpsc::sync_channel(256);
    let server = control::start(&dir, tx).unwrap();
    let release = Arc::new(AtomicBool::new(false));
    let stopping = Arc::new(AtomicBool::new(false));
    let pending_count = Arc::new(AtomicUsize::new(0));
    let first_responses = Arc::new(AtomicUsize::new(0));
    let worker = {
        let release = release.clone();
        let stopping = stopping.clone();
        let pending_count = pending_count.clone();
        thread::spawn(move || {
            let mut pending = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                if let Ok((_, BusCommand::Control(call))) =
                    rx.recv_timeout(Duration::from_millis(5))
                {
                    if call.request.method == "message.status" {
                        pending.push(call);
                        pending_count.fetch_add(1, Ordering::Release);
                    } else {
                        let _ = call.reply.send(Response::success(
                            &call.request.id,
                            json!({"accepted": true}),
                        ));
                    }
                }
                if release.load(Ordering::Acquire) {
                    for call in pending.drain(..) {
                        let _ = call.reply.send(Response::success(
                            &call.request.id,
                            status(vec![recipient(1, "replied", true)]),
                        ));
                    }
                }
            }
        })
    };
    let followers: Vec<_> = (0..100)
        .map(|id| {
            let dir = dir.clone();
            let first_responses = first_responses.clone();
            thread::spawn(move || {
                let mut first = true;
                follow_message(
                    &format!("follower-{id}"),
                    &json!(id),
                    &mut |request, _| {
                        let response = control::request(&dir, request);
                        if first {
                            first = false;
                            first_responses.fetch_add(1, Ordering::Release);
                        }
                        response
                    },
                    &mut thread::sleep,
                )
            })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pending_count.load(Ordering::Acquire) + first_responses.load(Ordering::Acquire) < 100 {
        assert!(
            Instant::now() < deadline,
            "all 100 followers must reach the saturated server"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(followers.iter().all(|f| !f.is_finished()));
    let outcomes: Vec<_> = [
        "agent.interrupt",
        "message.send",
        "state",
        "agent.dialog.answer",
    ]
    .into_iter()
    .map(|method| {
        control::request_with_timeout(
            &dir,
            &Request {
                id: method.into(),
                method: method.into(),
                params: json!({"agent": "worker", "text": "new work"}),
            },
            Duration::from_secs(2),
        )
    })
    .collect();
    release.store(true, Ordering::Release);
    let completed: Vec<_> = followers.into_iter().map(|f| f.join().unwrap()).collect();
    stopping.store(true, Ordering::Release);
    worker.join().unwrap();
    drop(server);
    std::fs::remove_dir_all(&dir).unwrap();
    for (method, outcome) in ["interrupt", "send", "state", "answer"]
        .into_iter()
        .zip(outcomes)
    {
        assert!(
            outcome.as_ref().is_ok_and(|r| r.ok),
            "{method} was starved by followers: {outcome:?}"
        );
    }
    assert!(
        completed.iter().all(|r| r.ok),
        "all 100 async followers must finish successfully"
    );
}

#[cfg(unix)]
#[test]
fn async_follower_reports_bus_down_after_a_saturated_server_stops() {
    use crate::messaging::control;
    use std::io::Write as _;
    use std::sync::mpsc;
    let dir = std::env::temp_dir().join(format!(
        "bcl-stop-{:x}-{:x}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
    let (commands, received) = mpsc::sync_channel(16);
    let server = control::start(&dir, commands).unwrap();
    let socket = dir.join("wait.sock");
    let mut pending = Vec::new();
    let mut peers = Vec::new();
    for id in 0..16 {
        let mut peer = crate::platform::ipc::connect_local_stream(&socket).unwrap();
        let request = Request {
            id: format!("held-{id}"),
            method: "message.status".into(),
            params: json!({"message": "19"}),
        };
        writeln!(peer, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        pending.push(received.recv_timeout(Duration::from_secs(5)).unwrap());
        peers.push(peer);
    }
    // Keep the extra overload response slot occupied while the follower connects.
    let overload_peer = crate::platform::ipc::connect_local_stream(&socket).unwrap();
    thread::sleep(Duration::from_millis(100));
    let (attempted, first_attempt) = mpsc::sync_channel(1);
    let (finished, outcome) = mpsc::sync_channel(1);
    let follower = {
        let dir = dir.clone();
        thread::spawn(move || {
            let mut first = true;
            let response = follow_message(
                "follow-after-stop",
                &json!(19),
                &mut |request, _| {
                    let response = control::request(&dir, request);
                    if first {
                        first = false;
                        let overloaded = match &response {
                            Ok(response) => response
                                .error
                                .as_ref()
                                .is_some_and(|error| error.code == "server_busy"),
                            Err(error) => error.starts_with("control_io:"),
                        };
                        attempted.send(overloaded).unwrap();
                    }
                    response
                },
                &mut thread::sleep,
            );
            finished.send(response).unwrap();
        })
    };
    let saw_overload = first_attempt.recv_timeout(Duration::from_secs(5));
    drop(server);
    drop(overload_peer);
    drop(peers);
    drop(pending);
    let response = outcome.recv_timeout(Duration::from_secs(8));
    if response.is_ok() {
        follower.join().unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(saw_overload.unwrap());
    let response = response.expect("a stopped poll endpoint must exhaust reconnect grace");
    assert_eq!(response.error.unwrap().code, "bus_down");
}

/// A failed `agent add` must say whether the add ran: failures before the
/// request was delivered prove it did not; a lost reply leaves it unknown.
#[test]
fn transport_errors_say_whether_the_command_ran() {
    let args = [
        "agent",
        "add",
        "--room",
        "7",
        "--name",
        "w1",
        "--provider",
        "claude",
        "--pwd",
        "/tmp",
    ];
    for (error, applied, says) in [
        (
            "control_unavailable: connect to an existing Bus instance: refused",
            json!(false),
            "agent.add was not delivered, so it did not run",
        ),
        (
            "request_timeout: request write deadline elapsed",
            json!(false),
            "agent.add was not delivered, so it did not run",
        ),
        (
            "control_unsent: Broken pipe (os error 32)",
            json!(false),
            "agent.add was not delivered, so it did not run",
        ),
        (
            "control_io: response connection closed or failed",
            json!("unknown"),
            "agent.add may have run",
        ),
        (
            "response_timeout: response deadline elapsed",
            json!("unknown"),
            "agent.add may have run",
        ),
    ] {
        let mut sends = 0;
        let (outcome, response) = run_captured(&args, |_, _| {
            sends += 1;
            Err(error.into())
        });
        assert!(outcome.is_err());
        assert_eq!(sends, 1, "{error}");
        assert_eq!(response["error"]["code"], "transport_error", "{error}");
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.starts_with(error), "{message}");
        assert!(message.contains(says), "{message}");
        assert_eq!(response["result"]["applied"], applied, "{error}");
    }
}

/// The follow-up `--text` send of `agent interrupt` is a one-shot mutation
/// too; its transport failure must still say whether the message was sent.
#[test]
fn interrupt_text_transport_error_says_whether_the_text_was_sent() {
    let (outcome, response) = run_captured(
        &["agent", "interrupt", "a4", "--text", "next task"],
        |request, _| match request.method.as_str() {
            "agent.interrupt" => Ok(Response::success(
                &request.id,
                json!({"room_id": 7, "agents": [{"agent_id": 4, "name": "a4", "outcome": "already_idle"}]}),
            )),
            _ => Err("control_io: response connection closed or failed".into()),
        },
    );
    assert!(outcome.is_err());
    assert_eq!(response["error"]["code"], "transport_error", "{response}");
    assert_eq!(response["result"]["applied"], "unknown", "{response}");
}

#[test]
fn agent_interrupt_still_reports_a_deletion_seen_after_failed_state_reads() {
    let (outcome, response, requests, _) = run_interrupt_reads(
        &["agent", "interrupt", "a4", "--text", "report status"],
        interrupting(4),
        [
            Ok(Response::failure("state", "server_busy", "try again")),
            Err("socket read timed out".into()),
            Ok(Response::success(
                "state",
                json!({"agents": [agent_row(5, "idle", false)]}),
            )),
        ],
    );
    assert_eq!(exit_code(&outcome), Some(1));
    assert_eq!(response["result"]["reason"], "agent a4 was deleted");
    assert!(requests.iter().all(|r| r.method != "message.send"));
    assert_eq!(requests.iter().filter(|r| r.method == "state").count(), 3);
}

#[test]
fn agent_interrupt_sends_text_only_to_the_agent_that_stopped_when_another_was_deleted() {
    let receipt = json!({"room_id": 7, "agents": [
        {"agent_id": 4, "name": "a4", "outcome": "interrupting", "key": "esc"},
        {"agent_id": 5, "name": "a5", "outcome": "interrupting", "key": "esc"},
    ]});
    let (outcome, response, requests, _) = run_interrupt_reads(
        &[
            "agent",
            "interrupt",
            "--room",
            "7",
            "--text",
            "report status",
        ],
        receipt,
        [
            Ok(Response::success(
                "state",
                json!({"agents": [agent_row(4, "working", true), agent_row(5, "working", true)]}),
            )),
            Ok(Response::failure("state", "coordinator_busy", "try again")),
            Ok(Response::success(
                "state",
                json!({"agents": [agent_row(4, "idle", false)]}),
            )),
        ],
    );
    assert_eq!(exit_code(&outcome), Some(1), "{response}");
    let send = requests
        .iter()
        .find(|r| r.method == "message.send")
        .unwrap();
    assert_eq!(send.params["to"], json!(["4"]));
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("agent a5 was deleted"));
}

#[test]
fn agent_interrupt_deadline_reason_reflects_the_last_successful_read_not_one_late_failure() {
    // Bus answered "still working" for nearly the whole deadline; only the
    // final read failed. The agent was observed working, not unobservable.
    let reads = std::iter::repeat_with(|| {
        Ok(Response::success(
            "state",
            json!({"agents": [agent_row(4, "working", true)]}),
        ))
    })
    .take(75)
    .chain(std::iter::repeat_with(|| {
        Ok(Response::failure("state", "server_busy", "try again"))
    }));
    let (outcome, response, _, _) =
        run_interrupt_reads(&["agent", "interrupt", "a4"], interrupting(4), reads);
    assert_eq!(exit_code(&outcome), Some(1));
    let reason = response["result"]["reason"].as_str().unwrap();
    assert!(reason.contains("still working"), "{reason}");
}
