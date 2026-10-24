use super::{claude_image_placeholders, claude_running_shells, is_bus_control, parse};
use crate::agents::providers::{hook_json::HookContract, spool::Parsed, ProviderKind};
use serde_json::{json, Value};
use std::path::Path;

#[test]
fn claude_adapter_observation_definitions_keep_the_live_callback_command_and_stop_failure() {
    let contract = HookContract::for_provider(ProviderKind::ClaudeCode);
    assert_eq!(
        contract.events,
        ["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"]
    );
    let command = contract.command(Path::new("/tmp/a path/it's bus"));
    assert!(command.ends_with(" --bus-callback claude-hook"));
    #[cfg(unix)]
    assert_eq!(
        command,
        "'/tmp/a path/it'\\''s bus' --bus-callback claude-hook"
    );
    assert_eq!(
        contract.entry(&command),
        json!({"hooks":[{"type":"command","command":command,"timeout":5}]})
    );
}

fn missing_field(key: &str) -> String {
    format!("Hook missing {key}; update the CLI and verify Bus hooks. No prompt will be retried.")
}

#[test]
fn claude_empty_stop_keeps_the_turn_with_null_missing_or_empty_text() {
    for text in [None, Some(Value::Null), Some(json!(""))] {
        let mut value = json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"p"});
        if let Some(text) = text {
            value["last_assistant_message"] = text;
        }
        assert_eq!(
            parse(&value).unwrap(),
            Parsed::Final {
                session: "s".into(),
                turn: "p".into(),
                text: String::new(),
            }
        );
    }
    for text in [json!(false), json!(7), json!([]), json!({})] {
        assert!(parse(
            &json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"p",
            "last_assistant_message":text})
        )
        .is_err());
    }
}

#[test]
fn claude_adapter_preserves_field_error_order_and_subagent_exclusion() {
    assert_eq!(
        parse(&json!({"agent_id":"child"})).unwrap_err(),
        missing_field("hook_event_name")
    );
    for value in [
        json!({"hook_event_name":"SubagentStop"}),
        json!({"hook_event_name":"Stop","agent_id":"child"}),
        json!({"hook_event_name":"Stop","agent_id":0}),
    ] {
        assert_eq!(parse(&value).unwrap(), Parsed::Ignore);
    }
    assert_eq!(
        parse(&json!({"hook_event_name":"Stop","agent_id":null})).unwrap_err(),
        missing_field("session_id")
    );
    assert_eq!(
        parse(&json!({"hook_event_name":"unknown","session_id":"s"})).unwrap_err(),
        missing_field("prompt_id")
    );
    assert_eq!(
        parse(&json!({"hook_event_name":"unknown","session_id":"s","prompt_id":"p"})).unwrap_err(),
        "Unsupported Bus hook event unknown"
    );
    for source in [Value::Null, json!(""), json!(" resume ")] {
        let expected = source.as_str().map(str::to_owned);
        assert_eq!(
            parse(&json!({"hook_event_name":"SessionStart","session_id":"s","source":source}))
                .unwrap(),
            Parsed::Session {
                session: "s".into(),
                source: expected
            }
        );
    }
}

fn started(prompt: &str) -> Parsed {
    parse(&json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt_id":"p","prompt":prompt})).unwrap()
}

fn prompt_fact(prompt: &str) -> Parsed {
    Parsed::Started {
        session: "s".into(),
        turn: "p".into(),
        prompt: prompt.into(),
    }
}

#[test]
fn claude_adapter_paste_normalization_preserves_images_and_exact_utf8_body() {
    let text = "  Reply: 你好\n\n- one\n- two\n ";
    let images = "[Image #1][Image #22]";
    let framed =
        format!("{images}\n\n<pasted_content id=\"a1\">\n{text}\n</pasted_content id=\"a1\">\n");
    assert_eq!(started(&framed), prompt_fact(&format!("{images}{text}")));
    assert_eq!(started(text), prompt_fact(text));
}

#[test]
fn claude_adapter_keeps_malformed_or_surrounded_paste_frames_literal() {
    for prompt in [
        "<pasted_content id=\"a1\">\ntext\n</pasted_content id=\"b2\">",
        "note\n<pasted_content id=\"a1\">\ntext\n</pasted_content id=\"a1\">",
        "<pasted_content id=\"a1\">\ntext\n</pasted_content id=\"a1\">\nsuffix",
        "<pasted_content id=\"\">\ntext\n</pasted_content id=\"\">",
        "<pasted_content id=\"a<1\">\ntext\n</pasted_content id=\"a<1\">",
        "<pasted_content id=\"a1\">text</pasted_content id=\"a1\">",
    ] {
        assert_eq!(started(prompt), prompt_fact(prompt));
    }
}

#[test]
fn paste_correlation_candidate_preserves_the_whole_body_and_rejects_ambiguous_frames() {
    use super::claude_paste_candidate;
    let frame =
        |body: &str| format!("<pasted_content id=\"a1\">\n{body}\n</pasted_content id=\"a1\">");
    assert_eq!(
        claude_paste_candidate(&format!("\n\n{}\n\na", frame("owned\n\nbody"))),
        Some("owned\n\nbody".into())
    );
    assert_eq!(
        claude_paste_candidate(&format!("[Image #1]\n{}\nextra", frame("owned"))),
        Some("[Image #1]owned".into())
    );
    for prompt in [
        "<pasted_content id=\"a1\">\nowned\n</pasted_content id=\"b2\">".to_owned(),
        "<pasted_content id=\"\">\nowned\n</pasted_content id=\"\">".to_owned(),
        format!("{}\n{}", frame("owned"), frame("other")),
        frame(&frame("owned")),
        format!("prefix\n{}", frame("owned")),
    ] {
        assert_eq!(claude_paste_candidate(&prompt), None, "{prompt}");
    }
}

#[test]
fn claude_adapter_image_prefix_counts_only_consecutive_ascii_numbered_placeholders() {
    let prefix = "[Image #01][Image #2]";
    assert_eq!(
        claude_image_placeholders(&format!("{prefix}你好")),
        (prefix.len(), 2)
    );
    assert_eq!(
        claude_image_placeholders("[Image #1][Image #x]"),
        ("[Image #1]".len(), 1)
    );
    for prompt in [
        "text[Image #1]",
        "[Image #]",
        "[Image #-1]",
        "[Image #١]",
        "[Image #1",
    ] {
        assert_eq!(claude_image_placeholders(prompt), (0, 0), "{prompt}");
    }
}

#[test]
fn claude_adapter_background_stop_holds_non_shell_tasks_and_crons_only() {
    for extra in [
        json!({"background_tasks":[{"task_id":"task-1","status":"running"}],"session_crons":[]}),
        json!({"background_tasks":[],"session_crons":[{"cron_id":"cron-1","status":"running"}]}),
        json!({"background_tasks":[{"id":"shell","type":"shell"},{"id":"agent","type":"local_agent"}]}),
    ] {
        let mut value = json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"p"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            parse(&value).unwrap(),
            Parsed::BackgroundPending {
                session: "s".into(),
                turn: "p".into()
            }
        );
    }
    for background_tasks in [
        json!([]),
        Value::Null,
        json!([{"id":"shell","type":"shell","status":"running"}]),
    ] {
        let value = json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"p","last_assistant_message":"Final review","background_tasks":background_tasks,"session_crons":[]});
        assert_eq!(
            parse(&value).unwrap(),
            Parsed::Final {
                session: "s".into(),
                turn: "p".into(),
                text: "Final review".into()
            }
        );
    }
}

#[test]
fn claude_adapter_stop_failure_reports_turn_identity_without_requiring_reply_text() {
    let value = json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p"});
    assert_eq!(
        parse(&value).unwrap(),
        Parsed::Failure {
            session: "s".into(),
            turn: "p".into(),
            message: "Provider turn failed; inspect its terminal.".into(),
            error: Some("unknown".into()),
        }
    );
    assert_eq!(
        parse(&json!({"hook_event_name":"StopFailure","session_id":"s"})).unwrap_err(),
        missing_field("prompt_id")
    );
}

/// Payload measured from Claude Code 2.1.296 when a 529 ended the turn: the
/// error text it shows is `last_assistant_message`, its category `error`.
#[test]
fn claude_stop_failure_names_the_api_error_that_ended_the_turn() {
    let failure = |value: Value| match parse(&value).unwrap() {
        Parsed::Failure { error, .. } => error.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        failure(json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p",
            "error":"server_error",
            "last_assistant_message":"API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment."})),
        "API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment. (server_error)"
    );
    // Only the category, then nothing at all.
    assert_eq!(
        failure(
            json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p","error":"rate_limit"})
        ),
        "rate_limit"
    );
    // Kept to one bounded line for statuses and logs.
    let long = format!("API Error:\n  {}", "é".repeat(400));
    let error = failure(
        json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p",
        "last_assistant_message":long}),
    );
    assert!(!error.contains('\n'));
    assert!(error.starts_with("API Error: éé"));
    assert!(error.ends_with('…'));
    assert_eq!(error.chars().count(), 301);
}

#[test]
fn claude_stop_failure_with_blank_or_non_text_error_fields_falls_back_to_the_category() {
    let failure = |value: Value| match parse(&value).unwrap() {
        Parsed::Failure { error, .. } => error.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        failure(
            json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p",
            "error":"server_error","last_assistant_message":" \n\t "})
        ),
        "server_error"
    );
    assert_eq!(
        failure(
            json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p",
            "error":{"type":"overloaded"},"last_assistant_message":"API Error: 529 Overloaded"})
        ),
        "API Error: 529 Overloaded"
    );
    let exact = "x".repeat(300);
    assert_eq!(
        failure(
            json!({"hook_event_name":"StopFailure","session_id":"s","prompt_id":"p",
            "last_assistant_message":exact.clone()})
        ),
        exact
    );
}

/// Live (2026-10-16): an orchestrator's every reply was held behind its own
/// `bus send --async` waits on workers, which run for minutes or hours. Bus's
/// waits are not work the reply depends on; other shells still hold it.
#[test]
fn claude_running_shells_leave_out_bus_send_and_wait_commands() {
    let shell = |id: &str, command: &str| json!({"id":id,"type":"shell","status":"running","description":"Run","command":command});
    let value = json!({"background_tasks":[
        shell("b1", "bus send --async --room qa --as orch --to worker --text 'go'"),
        shell("b2", "/Users/me/work/bus/target/debug/bus --dev send --room master --to orch --text hi"),
        shell("b3", "cd /tmp && bus --dev wait 42 --timeout 600"),
        shell("b4", "bus.exe wait 7"),
        shell("b5", "cargo test --locked"),
        shell("b6", "bus history --room 0 | grep send"),
        shell("b7", "sleep 45"),
    ]});
    assert_eq!(
        claude_running_shells(&value)
            .into_iter()
            .collect::<Vec<_>>(),
        ["b5", "b6", "b7"]
    );
    assert!(is_bus_control("bus send --to a --text b"));
    assert!(is_bus_control("BUS_DATA_DIR=/x ./bus --dev --yolo wait 3"));
    assert!(is_bus_control("bus state"));
    assert!(!is_bus_control("busybox send"));
    assert!(!is_bus_control("echo bus"));
}

#[test]
fn direct_bus_control_shells_do_not_hold_replies() {
    for command in [
        "bus history --room 0",
        "bus agent read worker --source recent --lines 20",
        "bus message status 19",
        "bus room notes 1 --text ready",
        "bus request recover 19 --confirm",
        "bus settings",
        "bus diagnostics",
        "bus sounds",
        "cd '/tmp/a b' && '/tmp/a b/bus' --dev send --async --to worker --text go",
    ] {
        assert!(is_bus_control(command), "Bus control only: {command}");
    }
    for command in [
        "bus agent read worker | python3 report.py",
        "bus history --room 0 && cargo test",
        "./rev_send.sh worker brief.md",
        "until bus state; do sleep 3; done; bus send --to worker --text go",
        "bus e2e",
        "bus tui start",
        "bus resume --last",
        "bus --unknown-option wait 19",
    ] {
        assert!(
            !is_bus_control(command),
            "Work or opaque command: {command}"
        );
    }
}

#[test]
fn async_dispatch_file_arguments_and_completion_echo_do_not_hold_replies() {
    for command in [
        r#"bus send --to worker --async --text "$(cat brief.md)""#,
        r#"cd /tmp; ./bus send --async --to worker --text "$(cat 'brief with spaces.md')" > out.json 2>&1; echo sent"#,
        r#"bus send --to worker --async --text $(cat brief.md)"#,
        r#"bus send --to worker --async --text go >/dev/null; echo sent"#,
    ] {
        assert!(is_bus_control(command), "dispatch only: {command}");
    }
}

#[test]
fn async_dispatch_wrappers_cannot_hide_real_work() {
    for command in [
        r#"bus send --async --text "$(cat brief.md; cargo test)""#,
        r#"bus send --async --text "$(cat $(generate-brief))""#,
        r#"bus send --async --text "$(cat <(cargo test))""#,
        r#"bus send --async --text "$(cat)""#,
        r#"bus send --async --text "$(cat -)""#,
        r#"bus send --async --text "$(cat brief.md)"; cargo test"#,
        r#"bus send --async --text "$(cat brief.md)"; echo "$(cargo test)""#,
        r#"bus send --async --text "$(cat brief.md)" | tee out.json"#,
        r#"bus send --async --text "$(cat 'brief.md)""#,
    ] {
        assert!(!is_bus_control(command), "must hold work: {command}");
    }
}

/// A background shell that runs real work and only then reports over Bus
/// (`cargo build && bus send ...`) is work the reply waits on: its exit
/// wakes the turn with the build's outcome, so it still holds the reply.
#[test]
fn claude_running_shells_keep_work_that_ends_with_a_bus_send() {
    let value = json!({"background_tasks":[
        {"id":"b1","type":"shell","status":"running","description":"Build","command":"cargo build --release && bus send --to qa --text 'build ready'"},
    ]});
    assert_eq!(
        claude_running_shells(&value)
            .into_iter()
            .collect::<Vec<_>>(),
        ["b1"]
    );
}

#[test]
fn bus_wait_shells_preserve_work_on_either_side_and_quoted_message_text() {
    for command in [
        "bus wait 7 && cargo build",
        "cargo build; bus send --to qa --text ready",
        "echo bus send --to qa --text ready",
        "bus send --to qa --text \"$(cargo build)\"",
        "bus send --to qa --text `cargo build`",
        "(bus wait 7)",
        "cd /tmp",
    ] {
        assert!(!is_bus_control(command), "must hold work: {command}");
    }
    for command in [
        "cd /tmp && BUS_DATA_DIR=/scratch bus --dev wait 7",
        "bus wait 7; bus send --to qa --text 'ready && checked; a | b'",
        "bus send --to qa --text 'literal $(cargo build)'",
        "bus send --to qa --text ready\\;checked",
    ] {
        assert!(is_bus_control(command), "only Bus waits: {command}");
    }
}

/// A stream redirection such as `2>&1` belongs to the Bus command it follows;
/// its `&` is not a command separator, so the shell is still only a Bus wait.
#[test]
fn bus_wait_shells_with_stream_redirections_are_still_bus_waits() {
    for command in [
        "bus send --to qa --async --text ready 2>&1",
        "bus wait --message 7 --timeout 600 2>&1",
        "bus wait 7 >&2 <&0",
        "bus wait 7 &>/tmp/bus-wait.log",
        "bus wait 7 >|/tmp/bus-wait.log",
    ] {
        assert!(is_bus_control(command), "only Bus waits: {command}");
    }
}

/// Real work after a redirected Bus wait still holds the reply: only the
/// redirection's own `&` or `|` is joined to the command, never a separator.
#[test]
fn bus_wait_shells_keep_work_after_a_stream_redirection() {
    for command in [
        "bus wait 7 2>&1 && cargo build",
        "bus wait 7 2>&1&& cargo build",
        "bus wait 7 >&2; cargo build",
        "bus wait 7 2>&1 | tee /tmp/log",
        "bus wait 7 &>/tmp/log & cargo build",
        "bus wait 7 >|/tmp/log || cargo build",
        "bus wait 7 &&>/tmp/log cargo build",
    ] {
        assert!(!is_bus_control(command), "must hold work: {command}");
    }
}

/// The repository shim is a wrapper, retained for foreground controls.
/// Background dispatches must name the Bus executable directly.
#[test]
fn repository_dev_control_dispatch_is_a_wrapper_and_stays_held() {
    assert!(!is_bus_control(
        "./run dev-control send --room work --as orch --to worker --async --text go"
    ));
}

/// Inside double quotes a shell keeps a backslash that does not precede
/// `$`, backtick, `"`, `\` or a newline, so a Windows path stays intact.
#[test]
fn double_quoted_windows_bus_path_is_a_direct_bus_control() {
    assert!(is_bus_control(
        r#""C:\Users\dev\bin\bus.exe" send --to worker --async --text go"#
    ));
}

/// A backslash-newline is a line continuation that the shell removes.
#[test]
fn line_continued_bus_controls_are_direct() {
    for command in [
        "bus send --room work --as orch --to worker --async \\\n  --text go",
        "'/tmp/a b/bus' --dev \\\n  send --to worker --async --text go",
    ] {
        assert!(is_bus_control(command), "Bus control only: {command:?}");
    }
}

#[test]
fn line_continuations_do_not_hide_real_work_around_bus_controls() {
    for command in [
        "cargo build \\\n && bus wait 7",
        "bus wait 7 \\\n && cargo build",
        "\"C:\\tools\\bus.exe\" wait 7 | cargo build",
    ] {
        assert!(!is_bus_control(command), "must hold work: {command:?}");
    }
}

#[test]
fn prompt_origin_requires_matching_provider_metadata_in_a_bounded_tail() {
    let root = crate::utils::test_temp::unique_temp_path("claude-scheduled");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("transcript.jsonl");
    let hook = json!({"session_id":"s","prompt_id":"p","transcript_path":path});
    let row = json!({"type":"user","sessionId":"s","promptId":"p","isMeta":true,
        "promptSource":"system","turnOrigin":"scheduled","scheduledTaskId":"cron-1"});
    std::fs::write(&path, format!("{row}\n")).unwrap();
    assert_eq!(
        super::prompt_origin(&hook),
        Some(super::PromptOrigin::Scheduled)
    );
    for (key, value) in [
        ("sessionId", json!("other-session")),
        ("promptId", json!("other-turn")),
        ("isMeta", json!(false)),
        ("promptSource", json!("user")),
        ("turnOrigin", json!("user")),
        ("scheduledTaskId", json!("")),
    ] {
        let mut changed = row.clone();
        changed[key] = value;
        std::fs::write(&path, format!("{changed}\n")).unwrap();
        let expected = if matches!(key, "sessionId" | "promptId") {
            None
        } else {
            Some(super::PromptOrigin::Other)
        };
        assert_eq!(super::prompt_origin(&hook), expected, "{key}");
    }
    // Later tool-result rows share the turn id but carry no prompt origin.
    let tool_result = json!({"type":"user","sessionId":"s","promptId":"p",
        "message":{"role":"user","content":[{"type":"tool_result","content":"done"}]}});
    std::fs::write(&path, format!("{row}\n{tool_result}\n")).unwrap();
    assert_eq!(
        super::prompt_origin(&hook),
        Some(super::PromptOrigin::Scheduled)
    );
    let mut large = vec![b'x'; 2 * 1024 * 1024 + 10];
    large.extend_from_slice(format!("\n{row}\n{{partial").as_bytes());
    std::fs::write(&path, large).unwrap();
    assert_eq!(
        super::prompt_origin(&hook),
        Some(super::PromptOrigin::Scheduled)
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(super::prompt_origin(&hook), None);
    assert_eq!(super::prompt_origin(&json!({})), None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dispatch_completion_echo_cannot_smuggle_real_work() {
    for command in [
        r#"bus send --to w --async --text "$(cat brief.md)"; echo sent >(cargo test)"#,
        r#"bus send --to w --async --text "$(cat brief.md)"; echo sent; exec cargo test"#,
        "bus send --to w --async --text \"$(cat <<'EOF'\ncargo test\nEOF\n)\"",
        r#"bus send --to w --async --text "$(cat brief.md)"; echo "${RESULT:=$(cargo test)}""#,
        r#"echo sent; cargo test"#,
        r#"$(cat runner.txt) send --to w --async --text go"#,
        r#"bus $(cat subcommand.txt) --to w"#,
    ] {
        assert!(!is_bus_control(command), "must hold work: {command}");
    }
}

#[test]
fn dispatch_reading_several_brief_files_or_assigning_one_does_not_hold() {
    for command in [
        r#"bus send --to w --async --text "$(cat header.md brief.md)" > out.json 2>&1; echo sent"#,
        r#"BRIEF="$(cat brief.md)" bus send --to w --async --text "$BRIEF""#,
        r#"bus send --to w --async --text '$(cargo test)'; echo sent"#,
    ] {
        assert!(is_bus_control(command), "dispatch only: {command}");
    }
}

// The classifier documents stdin reads as opaque work.
#[test]
fn dispatch_reading_stdin_through_a_device_path_still_holds() {
    for command in [
        r#"bus send --async --text "$(cat /dev/stdin)""#,
        r#"bus send --async --text "$(cat /dev/fd/0)""#,
    ] {
        assert!(!is_bus_control(command), "must hold work: {command}");
    }
}

// A shell comment inside the substitution hides the real closing parenthesis.
#[test]
fn file_substitution_with_a_comment_cannot_hide_real_work() {
    let command = "bus send --async --to w --text \"$(cat brief.md #)\ncargo test)\"";
    assert!(!is_bus_control(command), "must hold work: {command}");
}

#[test]
fn task_notifications_require_a_whole_envelope_but_not_a_task_id() {
    assert_eq!(
        super::task_notification_body(" \n<task-notification>finished</task-notification>\n"),
        Some("finished")
    );
    assert_eq!(
        super::task_notification_body(
            "<task-notification><task-id>b1</task-id></task-notification>"
        ),
        Some("<task-id>b1</task-id>")
    );
    for prompt in [
        "<task-notification>unfinished",
        "prefix <task-notification>done</task-notification>",
        "<task-notification>done</task-notification> suffix",
    ] {
        assert_eq!(super::task_notification_body(prompt), None, "{prompt}");
    }
}
