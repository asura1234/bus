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

#[test]
fn agy_completion_handles_torn_multibyte_utf8_tails_wakeups_and_large_transcripts() {
    let temp = TestTemp::new("agy-torn-utf8");
    let user = "{\"step_index\":4,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"<USER_REQUEST>\\ntask\\n</USER_REQUEST>\"}\n";

    let ascii_path = temp.path().join("ascii.jsonl");
    let mut ascii = user.as_bytes().to_vec();
    ascii.extend_from_slice(b"{\"step_index\":5,\"source\":\"MODEL\",\"type\":\"PLANNER_RES");
    std::fs::write(&ascii_path, &ascii).unwrap();
    assert_eq!(
        read_turn_completion(&ascii_path, "4"),
        Ok(TurnResolution::Pending)
    );

    let utf8_path = temp.path().join("utf8.jsonl");
    let full = "{\"step_index\":5,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"Done \u{2014} all green\"}\n";
    let cut = full.find('\u{2014}').unwrap() + 2;
    let mut torn = user.as_bytes().to_vec();
    torn.extend_from_slice(&full.as_bytes()[..cut]);
    std::fs::write(&utf8_path, &torn).unwrap();
    assert_eq!(
        read_turn_completion(&utf8_path, "4"),
        Ok(TurnResolution::Pending)
    );

    let wake_rows = [
        r#"{"step_index":82,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nA\n</USER_REQUEST>\n<ADDITIONAL_METADATA>x</ADDITIONAL_METADATA>"}"#,
        r#"{"step_index":83,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"invoke_subagent"}]}"#,
        r#"{"step_index":84,"source":"MODEL","type":"GENERIC","status":"DONE","content":"ok"}"#,
        r#"{"step_index":85,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"waiting"}"#,
        r#"{"step_index":86,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","content":"wake"}"#,
        r#"{"step_index":87,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"final A"}"#,
        r#"{"step_index":88,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nB\n</USER_REQUEST>"}"#,
        r#"{"step_index":89,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nC\n</USER_REQUEST>"}"#,
        r#"{"step_index":90,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"final BC"}"#,
    ];
    let wake_path = temp.path().join("wake.jsonl");
    std::fs::write(&wake_path, format!("{}\n", wake_rows.join("\n"))).unwrap();
    assert_eq!(
        read_turn_completion(&wake_path, "82"),
        Ok(TurnResolution::Final {
            turn_id: "82".into(),
            text: "final A".into(),
            prompts: vec!["A".into()],
        })
    );
    assert_eq!(
        read_turn_completion(&wake_path, "88"),
        Ok(TurnResolution::Final {
            turn_id: "88".into(),
            text: "final BC".into(),
            prompts: vec!["B".into(), "C".into()],
        })
    );

    // > 8 MiB transcript exercises both the rewind fallback (turn 0 before filler)
    // and the 8 MiB tail fast-path (turn 2 after filler).
    let large_path = temp.path().join("large.jsonl");
    let filler = "x".repeat(9 * 1024 * 1024);
    let large_body = format!(
        concat!(
            "{{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"<USER_REQUEST>\\nearly\\n</USER_REQUEST>\"}}\n",
            "{{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"{filler}\"}}\n",
            "{{\"step_index\":2,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"<USER_REQUEST>\\nlate\\n</USER_REQUEST>\"}}\n",
            "{{\"step_index\":3,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"tail ok\"}}\n",
        ),
        filler = filler
    );
    std::fs::write(&large_path, large_body).unwrap();
    assert_eq!(
        read_turn_completion(&large_path, "0"),
        Ok(TurnResolution::Final {
            turn_id: "0".into(),
            text: filler,
            prompts: vec!["early".into()],
        })
    );
    assert_eq!(
        read_turn_completion(&large_path, "2"),
        Ok(TurnResolution::Final {
            turn_id: "2".into(),
            text: "tail ok".into(),
            prompts: vec!["late".into()],
        })
    );
}

#[test]
fn agy_statusline_selects_3p_quota_resolves_relative_settings_and_respects_global_sandbox() {
    let payload_3p = json!({
        "session_id": "s",
        "model": {"id": "claude-opus-5-5-high", "display_name": "Claude Opus 5.5"},
        "quota": {
            "gemini-5h": {"remaining_fraction": 1.0, "reset_in_seconds": 3600},
            "gemini-weekly": {"remaining_fraction": 1.0, "reset_in_seconds": 86400},
            "3p-5h": {"remaining_fraction": 0.0, "reset_in_seconds": 1800},
            "3p-weekly": {"remaining_fraction": 0.5, "reset_in_seconds": 86400}
        }
    });
    let windows_3p = statusline::windows(&payload_3p, 1_700_000_000_000);
    assert!((windows_3p.five_hour.unwrap().used_percent - 100.0).abs() < 1e-6);
    assert!((windows_3p.weekly.unwrap().used_percent - 50.0).abs() < 1e-6);

    let payload_gemini = json!({
        "session_id": "s",
        "model": {"id": "gemini-4-carbon-high"},
        "quota": {
            "gemini-5h": {"remaining_fraction": 0.75, "reset_in_seconds": 3600},
            "3p-5h": {"remaining_fraction": 0.0, "reset_in_seconds": 1800}
        }
    });
    let windows_gemini = statusline::windows(&payload_gemini, 1_700_000_000_000);
    assert!((windows_gemini.five_hour.unwrap().used_percent - 25.0).abs() < 1e-6);

    let temp = TestTemp::new("agy-rel-settings");
    let launch_cwd = temp.path().join("launch-cwd");
    let spool = temp.path().join("spool");
    std::fs::create_dir_all(&launch_cwd).unwrap();
    std::fs::create_dir_all(&spool).unwrap();
    std::fs::write(
        launch_cwd.join("user-agy-settings.json"),
        br#"{"agentMode":"accept-edits"}"#,
    )
    .unwrap();
    let installed = statusline::install(
        &spool,
        &launch_cwd,
        std::path::Path::new("/opt/bus/bin/bus"),
        &["user-agy-settings.json".to_owned()],
        false,
    )
    .unwrap();
    let merged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&installed).unwrap()).unwrap();
    assert_eq!(merged["agentMode"], "accept-edits");

    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreHome(std::env::var_os("HOME"));
    let home = temp.path().join("fake-home");
    let global_dir = home.join(".gemini/antigravity-cli");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(
        global_dir.join("settings.json"),
        br#"{"enableTerminalSandbox":true}"#,
    )
    .unwrap();
    std::env::set_var("HOME", &home);
    let spool_global = temp.path().join("spool-global");
    std::fs::create_dir_all(&spool_global).unwrap();
    let path_global = statusline::install(
        &spool_global,
        &launch_cwd,
        std::path::Path::new("/opt/bus/bin/bus"),
        &[],
        false,
    )
    .unwrap();
    let written_global: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path_global).unwrap()).unwrap();
    assert_ne!(written_global["enableTerminalSandbox"], false);

    // Corrupt settings file preserves underlying cause during cold resume.
    std::fs::write(spool.join("agy-settings.json"), b"{not-json").unwrap();
    let err = statusline::rebind(&spool, std::path::Path::new("/opt/bus/bin/bus")).unwrap_err();
    assert!(err.contains("cannot be read"), "{err}");
}

#[test]
fn agy_hooks_ignore_child_subagents_and_system_wakeup_continuations() {
    let temp = TestTemp::new("agy-subagent-hooks");
    let brain = temp.path().join("brain");
    let parent_id = "parent-conv";
    let child_id = "child-subagent";
    let parent_subagents = brain.join(parent_id).join(".system_generated/subagents");
    let parent_logs = brain.join(parent_id).join(".system_generated/logs");
    let child_logs = brain.join(child_id).join(".system_generated/logs");
    std::fs::create_dir_all(&parent_subagents).unwrap();
    std::fs::create_dir_all(&parent_logs).unwrap();
    std::fs::create_dir_all(&child_logs).unwrap();
    std::fs::write(
        parent_subagents.join(format!("{child_id}.json")),
        r#"{"conversationId":"child-subagent"}"#,
    )
    .unwrap();

    let parent_transcript = parent_logs.join("transcript_full.jsonl");
    std::fs::write(
        &parent_transcript,
        concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"<USER_REQUEST>\\nhi\\n</USER_REQUEST>\"}\n",
            "{\"step_index\":1,\"source\":\"SYSTEM\",\"type\":\"SYSTEM_MESSAGE\",\"status\":\"DONE\",\"content\":\"subagent done\"}\n",
        ),
    )
    .unwrap();
    let child_transcript = child_logs.join("transcript_full.jsonl");
    std::fs::write(
        &child_transcript,
        "{\"step_index\":0,\"source\":\"SYSTEM\",\"type\":\"SYSTEM_MESSAGE\",\"status\":\"DONE\",\"content\":\"task\"}\n",
    )
    .unwrap();

    let subagent_start = json!({
        "hookEventName": "SessionStart",
        "conversationId": child_id,
        "artifactDirectoryPath": brain.join(child_id),
        "transcriptPath": child_transcript,
        "lastUserInput": "",
    });
    assert_eq!(
        parse(ProviderKind::Agy, &subagent_start),
        Ok(Parsed::Ignore)
    );
    assert!(
        crate::agents::providers::agy::hooks::is_descendant_subagent(
            parent_id,
            child_id,
            &subagent_start
        )
    );

    let subagent_stop = json!({
        "hookEventName": "Stop",
        "conversationId": child_id,
        "artifactDirectoryPath": brain.join(child_id),
        "transcriptPath": child_transcript,
        "lastUserInput": "",
        "fullyIdle": true,
    });
    assert_eq!(parse(ProviderKind::Agy, &subagent_stop), Ok(Parsed::Ignore));

    // Parent wakeup on SYSTEM_MESSAGE at step_index 1 (initialNumSteps == 2) is ignored!
    let parent_wakeup = json!({
        "hookEventName": "PreInvocation",
        "conversationId": parent_id,
        "invocationNum": 0,
        "initialNumSteps": 2,
        "lastUserInput": "hi",
        "transcriptPath": parent_transcript,
    });
    assert_eq!(parse(ProviderKind::Agy, &parent_wakeup), Ok(Parsed::Ignore));

    // Parent user prompt at step_index 0 (initialNumSteps == 1) emits Started!
    let parent_user = json!({
        "hookEventName": "PreInvocation",
        "conversationId": parent_id,
        "invocationNum": 0,
        "initialNumSteps": 1,
        "lastUserInput": "hi",
        "transcriptPath": parent_transcript,
    });
    assert_eq!(
        parse(ProviderKind::Agy, &parent_user),
        Ok(Parsed::Started {
            session: parent_id.into(),
            turn: "0".into(),
            prompt: "hi".into(),
        })
    );
}

#[cfg(unix)]
#[test]
fn agy_project_hooks_exclude_generated_artifacts_below_and_outside_home() {
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreHome(std::env::var_os("HOME"));

    let temp = TestTemp::new("agy-home-exclude");
    let fake_home = temp.path().join("home");
    std::fs::create_dir_all(fake_home.join(".gemini/antigravity-cli")).unwrap();
    std::env::set_var("HOME", &fake_home);

    for rel in ["home/work/project", "outside/work/project"] {
        let repo = temp.path().join(rel);
        std::fs::create_dir_all(&repo).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .unwrap();
        assert!(init.success());

        project_hooks::install(&repo, std::path::Path::new("/opt/bus/bin/bus")).unwrap();
        std::fs::write(repo.join(".agents/user-settings.json"), "{}\n").unwrap();

        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["status", "--short", "--untracked-files=all"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let status = String::from_utf8(out.stdout).unwrap();
        assert!(
            status.contains(".agents/user-settings.json"),
            "user file should remain untracked in {rel}: {status}"
        );
        assert!(
            !status.contains(".agents/hooks.json"),
            "generated hooks.json was not excluded in {rel}: {status}"
        );
        assert!(
            !status.contains(".agents/.bus-hooks.lock"),
            "generated .bus-hooks.lock was not excluded in {rel}: {status}"
        );
    }
}

#[test]
fn agy_hook_transaction_reuses_current_files_and_rejects_unowned_or_oversized_resume() {
    let temp = TestTemp::new("agy-hook-transaction");
    let binary = std::path::Path::new("/opt/bus-a/bus");
    let path = project_hooks::install(temp.path(), binary).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000000)),
    )
    .unwrap();
    let modified = path.metadata().unwrap().modified().unwrap();
    project_hooks::install(temp.path(), binary).unwrap();
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    assert!(!project_hooks::rebind(temp.path(), binary).unwrap().1);
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    std::fs::write(&path, br#"{"team":{"Stop":[]}}"#).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(project_hooks::rebind(temp.path(), binary).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    project_hooks::install(temp.path(), binary).unwrap();
    let mut oversized = std::fs::read(&path).unwrap();
    oversized.resize(2 * 1024 * 1024 + 1, b' ');
    std::fs::write(&path, &oversized).unwrap();
    assert!(project_hooks::rebind(temp.path(), binary)
        .unwrap_err()
        .contains("too large"));
    assert_eq!(std::fs::read(&path).unwrap(), oversized);
}

#[cfg(unix)]
#[test]
fn agy_git_exclude_does_not_interpret_grok_global_registry_files() {
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreHome(std::env::var_os("HOME"));
    let temp = TestTemp::new("agy-exclude-policy");
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".gemini/antigravity-cli")).unwrap();
    std::fs::write(
        home.join(".gemini/hooks-paths"),
        "/this/grok/registry/entry/does/not/exist\n",
    )
    .unwrap();
    std::env::set_var("HOME", &home);
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    project_hooks::install(&root, std::path::Path::new("/opt/bus/bin/bus")).unwrap();
    let excluded = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert!(excluded.lines().any(|line| line == "/.agents/hooks.json"));
    assert!(excluded
        .lines()
        .any(|line| line == "/.agents/.bus-hooks.lock"));
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
}
