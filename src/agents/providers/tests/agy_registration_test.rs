use super::agy::{
    completion::{read_turn_completion, resolve_transcript_path, scan_lines, TurnResolution},
    hooks::{stop_verdict, StopVerdict},
    project_hooks, statusline,
};
use super::{launch, parse, spool::Parsed, ProviderKind};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

struct TestTemp(std::path::PathBuf);
impl TestTemp {
    fn new(prefix: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "bus-{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TestTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn agy_hooks_parse_session_outer_turn_and_reconcile_stop() {
    let session = json!({
        "hookEventName": "SessionStart",
        "conversationId": "11111111-2222-3333-4444-555555555555",
        "transcriptPath": "/tmp/brain/11111111-2222-3333-4444-555555555555/.system_generated/logs/transcript_full.jsonl",
    });
    assert_eq!(
        parse(ProviderKind::Agy, &session).unwrap(),
        Parsed::Session {
            session: "11111111-2222-3333-4444-555555555555".into(),
            source: Some("startup".into()),
        }
    );

    let outer = json!({
        "hookEventName": "PreInvocation",
        "conversationId": "11111111-2222-3333-4444-555555555555",
        "invocationNum": 0,
        "initialNumSteps": 5,
        "lastUserInput": "Run checks",
    });
    assert_eq!(
        parse(ProviderKind::Agy, &outer).unwrap(),
        Parsed::Started {
            session: "11111111-2222-3333-4444-555555555555".into(),
            turn: "4".into(),
            prompt: "Run checks".into(),
        }
    );

    // Tool-loop or steered continuation inside the same outer turn is ignored as a new Started.
    let inner = json!({
        "hookEventName": "PreInvocation",
        "conversationId": "11111111-2222-3333-4444-555555555555",
        "invocationNum": 1,
        "initialNumSteps": 7,
        "lastUserInput": "Run checks",
    });
    assert_eq!(parse(ProviderKind::Agy, &inner).unwrap(), Parsed::Ignore);

    let stop = json!({
        "hookEventName": "Stop",
        "conversationId": "11111111-2222-3333-4444-555555555555",
        "terminationReason": "NO_TOOL_CALL",
        "fullyIdle": true,
    });
    assert_eq!(
        parse(ProviderKind::Agy, &stop).unwrap(),
        Parsed::ReconcileSession {
            session: "11111111-2222-3333-4444-555555555555".into(),
        }
    );
    assert_eq!(stop_verdict(&stop).unwrap(), StopVerdict::Completed);

    let bg = json!({
        "hookEventName": "Stop",
        "conversationId": "11111111-2222-3333-4444-555555555555",
        "terminationReason": "NO_TOOL_CALL",
        "fullyIdle": false,
    });
    assert_eq!(stop_verdict(&bg).unwrap(), StopVerdict::BackgroundPending);

    // Foreign callback filter allows native AGY camelCase and rejects Grok dual-envelope traffic.
    assert!(!super::grok::hooks::is_foreign_callback(
        ProviderKind::Agy,
        &outer
    ));
    let grok_dual = json!({
        "hookEventName": "session_start",
        "hook_event_name": "SessionStart",
        "sessionId": "grok-1",
    });
    assert!(super::grok::hooks::is_foreign_callback(
        ProviderKind::Agy,
        &grok_dual
    ));
}

#[test]
fn agy_transcript_scanner_extracts_steered_prompts_and_defers_torn_writes() {
    let lines = [
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\ninitial onboarding\n</USER_REQUEST>\n<ADDITIONAL_METADATA>{}</ADDITIONAL_METADATA>"}"#,
        r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"Ready."}"#,
        r#"{"step_index":4,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nfirst prompt\n</USER_REQUEST>\n<ADDITIONAL_METADATA>{}</ADDITIONAL_METADATA>"}"#,
        r#"{"step_index":5,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"Running sleep","tool_calls":[{"name":"run_command"}]}"#,
        r#"{"step_index":6,"source":"TOOL","type":"TOOL_RESPONSE","status":"DONE","content":"ok"}"#,
        r#"{"step_index":7,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nsteered correction\n</USER_REQUEST>"}"#,
        r#"{"step_index":8,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"Combined answer"}"#,
        r#"{"step_index":9,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nlater outer turn\n</USER_REQUEST>"}"#,
        r#"{"step_index":10,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"Later answer"}"#,
    ];
    let result = scan_lines(
        lines.into_iter().map(|line| Ok::<_, &str>(line.to_owned())),
        4,
        "4",
    )
    .unwrap();
    assert_eq!(
        result,
        TurnResolution::Final {
            turn_id: "4".into(),
            text: "Combined answer".into(),
            prompts: vec!["first prompt".into(), "steered correction".into()],
        }
    );

    // Torn JSONL trailing write stays Pending.
    let torn = [
        r#"{"step_index":4,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\ntask\n</USER_REQUEST>"}"#,
        r#"{"step_index":5,"source":"MODEL","type":"PLANNER_RES"#,
    ];
    assert_eq!(
        scan_lines(
            torn.into_iter().map(|line| Ok::<_, &str>(line.to_owned())),
            4,
            "4"
        )
        .unwrap(),
        TurnResolution::Pending
    );

    // Transcript path must belong to the callback's conversationId.
    assert!(resolve_transcript_path(
        "/tmp/brain/other-session/.system_generated/logs/transcript_full.jsonl",
        "expected-session"
    )
    .is_err());

    let temp = TestTemp::new("agy-transcript");
    let session_dir = temp.path().join("sess-1/.system_generated/logs");
    std::fs::create_dir_all(&session_dir).unwrap();
    let full = session_dir.join("transcript_full.jsonl");
    std::fs::write(
        &full,
        concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"<USER_REQUEST>\\nhello\\n</USER_REQUEST>\"}\n",
            "{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"world\"}\n"
        ),
    )
    .unwrap();
    let resolved = resolve_transcript_path(
        session_dir.join("transcript.jsonl").to_str().unwrap(),
        "sess-1",
    )
    .unwrap();
    assert_eq!(resolved, full);
    assert_eq!(
        read_turn_completion(&resolved, "0").unwrap(),
        TurnResolution::Final {
            turn_id: "0".into(),
            text: "world".into(),
            prompts: vec!["hello".into()],
        }
    );
}

#[test]
fn agy_project_hooks_preserve_other_named_blocks_and_rebind_binary() {
    let temp = TestTemp::new("agy-hooks");
    let root = temp.path();
    let hooks_dir = root.join(".agents");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let path = hooks_dir.join("hooks.json");
    // Include UTF-8 BOM and an existing team hook block.
    let mut initial = b"\xef\xbb\xbf".to_vec();
    initial.extend_from_slice(
        serde_json::to_vec(&json!({
            "team-lint": {
                "Stop": [{"type": "command", "command": "echo lint"}]
            }
        }))
        .unwrap()
        .as_slice(),
    );
    std::fs::write(&path, initial).unwrap();

    let bin_a = std::path::Path::new("/opt/bus-a/bus");
    let bin_b = std::path::Path::new("/opt/bus-b/bus");
    project_hooks::install(root, bin_a).unwrap();
    assert!(project_hooks::hooks_are_current(&path, bin_a));
    assert!(!project_hooks::hooks_are_current(&path, bin_b));

    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc["team-lint"]["Stop"][0]["command"], "echo lint");
    assert!(project_hooks::has_owned_hooks(&doc));

    let (_, changed) = project_hooks::rebind(root, bin_b).unwrap();
    assert!(changed);
    assert!(project_hooks::hooks_are_current(&path, bin_b));
}

#[test]
fn agy_launch_options_adoption_validation_settings_and_yolo() {
    let uuid = "12345678-1234-4321-abcd-1234567890ab";
    let mut args = vec![
        "--effort".to_owned(),
        "high".to_owned(),
        format!("--conversation={uuid}"),
    ];
    assert_eq!(
        launch::take_adopted_session(ProviderKind::Agy, &mut args)
            .unwrap()
            .as_deref(),
        Some(uuid)
    );
    assert_eq!(args, vec!["--effort", "high"]);

    let mut invalid = vec!["--conversation".to_owned(), "not-a-uuid".to_owned()];
    assert!(launch::take_adopted_session(ProviderKind::Agy, &mut invalid).is_err());

    for bad in [
        vec!["-p"],
        vec!["--print", "hi"],
        vec!["-i", "hi"],
        vec!["--continue"],
        vec!["--remote-agent"],
        vec!["acp"],
        vec!["models"],
        vec!["--", "positional"],
    ] {
        let bad: Vec<String> = bad.into_iter().map(str::to_owned).collect();
        assert!(
            launch::validate_args(ProviderKind::Agy, &bad).is_err(),
            "expected {bad:?} to be rejected"
        );
    }

    let saved = vec![
        "--effort".to_owned(),
        "high".to_owned(),
        "--settings".to_owned(),
        r#"{"agentMode":"accept-edits"}"#.to_owned(),
    ];
    assert_eq!(
        launch::resumable_options(ProviderKind::Agy, &saved),
        vec!["--effort", "high"]
    );

    let temp = TestTemp::new("agy-settings");
    let settings_path = statusline::install(
        temp.path(),
        std::path::Path::new("/opt/bus/bin/bus"),
        &[r#"{"agentMode":"accept-edits"}"#.to_owned()],
        false,
    )
    .unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    assert_eq!(written["queuedMessages"], "send-immediately");
    assert_eq!(written["allowNonWorkspaceAccess"], true);
    assert_eq!(written["enableTerminalSandbox"], false);
    assert_eq!(written["agentMode"], "accept-edits");
    assert_eq!(written["statusLine"]["enabled"], true);

    let quota_sample = json!({
        "quota": {
            "gemini-5h": {
                "remaining_fraction": 0.75,
                "reset_time": "2026-11-18T20:00:00Z",
                "reset_in_seconds": 3600
            },
            "gemini-weekly": {
                "remaining_fraction": 0.4,
                "reset_in_seconds": 86400
            }
        }
    });
    let parsed_windows = statusline::windows(&quota_sample, 1_700_000_000_000);
    assert!((parsed_windows.five_hour.unwrap().used_percent - 25.0).abs() < 1e-6);
    assert!((parsed_windows.weekly.unwrap().used_percent - 60.0).abs() < 1e-6);

    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("BUS_YOLO", value),
                None => std::env::remove_var("BUS_YOLO"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = Restore(std::env::var_os("BUS_YOLO"));
    std::env::set_var("BUS_YOLO", "1");
    let mut yolo_args = vec!["--effort".to_owned(), "high".to_owned()];
    launch::apply_run_permissions(ProviderKind::Agy, &mut yolo_args);
    assert_eq!(
        yolo_args,
        vec!["--dangerously-skip-permissions", "--effort", "high"]
    );
}
