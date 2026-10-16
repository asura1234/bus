use super::*;

// Minimized from a captured Codex permission screen:
// a wrapped option followed by an indented shell redirection continuation.
const CODEX_SOFT_WRAPPED_PERMISSION: &str =
    include_str!("fixtures/codex_soft_wrapped_permission.txt");

#[tokio::test]
async fn unwrapped_ansi_keeps_indentation_of_cursor_positioned_dialog_rows() {
    // ratatui-style diff rendering never writes untouched blank cells, so the
    // indentation of each row exists only as cursor positioning.
    let (mut app, _) = app_with_dialog(b"", Some("session"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(
        160,
        12,
        b"\x1b[1;3HWould you like to run the following command?\
          \x1b[2;3H$ cargo test --locked --bin bus dialog_probe > /tmp/dialog-probe.log 2>&1\
          \x1b[3;1H\xe2\x80\xba 1. Yes, proceed (y)\
          \x1b[4;3H2. Yes, and don't ask again for commands that start with `cargo test dialog_probe\
          \x1b[5;6H> /tmp/dialog-probe.log 2>&1` (p)\
          \x1b[6;3H3. No, and tell Codex what to do differently (esc)\
          \x1b[7;3HPress enter to confirm or esc to cancel",
    );
    app.state.insert_test_runtime(pane, runtime);
    let dialog = observe_dialog(&mut app)
        .dialog
        .expect("cursor-positioned Codex permission dialog");
    assert_eq!(dialog.options.len(), 3);
    assert_eq!(
        dialog.options[1].label,
        "Yes, and don't ask again for commands that start with `cargo test dialog_probe > /tmp/dialog-probe.log 2>&1` (p)"
    );
}

#[tokio::test]
async fn soft_wrapped_highlighted_option_rejects_a_changed_command_suffix() {
    let screen = " Choose a policy\r\n\r\n   1. Allow once\r\n\x1b[7m   2. Allow commands that start with cargo test --locked --bin bus dialog_probe > /tmp/dialog-probe.log 2>&1\x1b[0m\r\n   3. Deny\r\n\r\n Enter to select · Esc to cancel\r\n";
    let (mut app, _) = app_with_dialog(b"", Some("session"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, mut writes) = crate::terminal::TerminalRuntime::test_with_channel(40, 24);
    runtime.test_process_pty_bytes(screen.as_bytes());
    app.state.insert_test_runtime(pane, runtime);
    let observed = observe_dialog(&mut app);
    let dialog = observed
        .dialog
        .as_ref()
        .expect("soft-wrapped highlighted choice");
    assert_eq!(dialog.options.len(), 3);
    assert_eq!(
        dialog
            .options
            .iter()
            .map(|option| option.selected)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(dialog.options[1].label, "Allow commands that start with cargo test --locked --bin bus dialog_probe > /tmp/dialog-probe.log 2>&1");

    let edited = screen.replace("dialog-probe.log", "different-target.log");
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{edited}").as_bytes());
    let current = observe_dialog(&mut app);
    assert_ne!(dialog.digest, current.dialog.as_ref().unwrap().digest);
    let rejected = chosen(&choose(
        &mut app,
        &observed,
        Some("session"),
        &dialog.digest,
        1,
    ));
    assert!(!rejected.written);
    assert_eq!(rejected.reason.as_deref(), Some("stale_or_changed_dialog"));
    assert!(writes.try_recv().is_err());

    let accepted = chosen(&choose(
        &mut app,
        &current,
        Some("session"),
        &current.dialog.as_ref().unwrap().digest,
        1,
    ));
    assert!(accepted.written);
    assert_eq!(accepted.keys, ["up", "enter"]);
    assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\x1b[A"));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"\r")
    );
}

#[tokio::test]
async fn codex_soft_wrapped_question_footer_is_observed_and_answerable() {
    let (mut app, _) = app_with_dialog(b"", Some("session"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, mut writes) = crate::terminal::TerminalRuntime::test_with_channel(40, 24);
    runtime.test_process_pty_bytes(CODEX_TEXT_DIALOG);
    app.state.insert_test_runtime(pane, runtime);
    let observed = observe_dialog(&mut app);
    let dialog = observed
        .dialog
        .as_ref()
        .expect("question with a soft-wrapped footer");
    assert_eq!(dialog.kind, AgentDialogKind::Question);
    assert_eq!(dialog.text, "What token should Bus use?");
    assert!(dialog.options.is_empty());
    let result = chosen(&app.handle_agent_dialog_answer(
        "answer".into(),
        answer_params(&observed, Some("probe"), false),
    ));
    assert!(result.written);
    assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"probe"));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"\r")
    );
}

#[tokio::test]
async fn codex_soft_wrapped_permission_is_observed_and_answerable_after_resize() {
    let (mut app, _) = app_with_dialog(b"", Some("session"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, mut writes) = crate::terminal::TerminalRuntime::test_with_channel(160, 45);
    runtime.test_process_pty_bytes(
        CODEX_SOFT_WRAPPED_PERMISSION
            .replace('\n', "\r\n")
            .as_bytes(),
    );
    app.state.insert_test_runtime(pane, runtime);
    assert_eq!(observe_dialog(&mut app).dialog.unwrap().options.len(), 3);
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .resize(45, 80, 0, 0);
    let observed = observe_dialog(&mut app);
    let dialog = observed
        .dialog
        .as_ref()
        .expect("visible permission after terminal reflow");
    assert_eq!(dialog.options.len(), 3);
    assert!(dialog.options[0].selected);
    assert_eq!(dialog.options[1].label, "Yes, and don't ask again for commands that start with `CARGO_INCREMENTAL=0 cargo test --locked --bin bus dialog_probe > /tmp/dialog-probe.log 2>&1` (p)");
    let result = chosen(&choose(
        &mut app,
        &observed,
        Some("session"),
        &dialog.digest,
        3,
    ));
    assert!(result.written);
    assert_eq!(result.keys, ["down", "down", "enter"]);
    assert_eq!(
        writes.recv().await.unwrap(),
        Bytes::from_static(b"\x1b[B\x1b[B")
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"\r")
    );
}

#[tokio::test]
async fn missed_blocker_screen_payload_is_excluded_by_dev_logging_policy() {
    let (mut app, _) = app_with_dialog(b"private-token=dialog-secret-value\r\n", None);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Claude), AgentState::Blocked);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered(crate::utils::logging::DEV_FILTER, || {
        let observed = observe_dialog(&mut app);
        assert!(observed.dialog.is_none());
    });
    let log = capture.text();
    assert!(
        log.contains("bus.dialog.observed"),
        "metadata must remain observable: {log}"
    );
    assert!(
        !log.contains("dialog-secret-value"),
        "dev logging must exclude screen payloads: {log}"
    );
    assert!(
        log.contains("screen_hash"),
        "snapshot identity remains available: {log}"
    );
    let trace = crate::utils::logging::test_capture::Capture::default();
    trace.run_filtered("trace", || {
        observe_dialog(&mut app);
    });
    assert!(
        !trace.text().contains("dialog-secret-value"),
        "even unrestricted trace must contain no screen payload"
    );
}

#[tokio::test]
async fn agent_dialog_answer_pastes_literal_text_then_enters_and_encodes_skip() {
    let (mut app, mut writes) = app_with_dialog(CODEX_TEXT_DIALOG, Some("session"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[?2004h");
    let observation = observe_dialog(&mut app);
    assert_eq!(
        observation.dialog.as_ref().unwrap().kind,
        AgentDialogKind::Question
    );
    let params = answer_params(&observation, Some("hello 世界"), false);
    let choice = chosen(&app.handle_agent_dialog_answer("answer".into(), params));
    assert!(choice.written);
    assert_eq!(choice.keys, ["enter"]);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from("\x1b[200~hello 世界\x1b[201~")
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"\r")
    );
    let skipped = chosen(
        &app.handle_agent_dialog_answer("skip".into(), answer_params(&observation, None, true)),
    );
    assert!(skipped.written);
    assert_eq!(skipped.keys, ["ctrl+]"]);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\x1d"));
}

#[tokio::test]
async fn agent_dialog_answer_sends_nothing_for_changed_identity_screen_or_invalid_text() {
    let (mut app, mut writes) = app_with_dialog(CODEX_TEXT_DIALOG, Some("session"));
    let observed = observe_dialog(&mut app);
    for field in ["terminal", "pane", "session", "digest"] {
        let mut params = answer_params(&observed, Some("token"), false);
        match field {
            "terminal" => params.expected_terminal_id = "wrong".into(),
            "pane" => params.expected_pane_id = "wrong".into(),
            "session" => params.expected_session_id = Some("wrong".into()),
            _ => params.expected_dialog_digest = "wrong".into(),
        }
        let result = app.handle_agent_dialog_answer("reject".into(), params);
        if field == "digest" {
            assert!(!chosen(&result).written);
        } else {
            assert!(
                serde_json::from_str::<crate::protocol::api::schema::ErrorResponse>(&result)
                    .is_ok()
            );
        }
        assert!(writes.try_recv().is_err());
    }
    for (text, skip) in [
        (Some("token"), true),
        (None, false),
        (Some(" "), false),
        (Some("a\x1d"), false),
        (Some("a\x1b[201~"), false),
    ] {
        let result =
            app.handle_agent_dialog_answer("invalid".into(), answer_params(&observed, text, skip));
        assert!(
            serde_json::from_str::<crate::protocol::api::schema::ErrorResponse>(&result).is_ok()
        );
        assert!(writes.try_recv().is_err());
    }
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let edited = format!(
        "\x1b[2J\x1b[H{}",
        String::from_utf8_lossy(CODEX_TEXT_DIALOG).replace("Type your answer", "developer edit")
    );
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(edited.as_bytes());
    let stale = chosen(
        &app.handle_agent_dialog_answer("edited".into(), answer_params(&observed, None, true)),
    );
    assert!(!stale.written);
    assert!(writes.try_recv().is_err());
    let (mut app, mut writes) = app_with_dialog(CLAUDE_BASH_DIALOG, None);
    let observed = observe_dialog(&mut app);
    let result = chosen(&app.handle_agent_dialog_answer(
        "choice".into(),
        answer_params(&observed, Some("token"), false),
    ));
    assert!(!result.written);
    assert_eq!(result.reason.as_deref(), Some("not_a_free_text_question"));
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn agent_dialog_choose_moves_from_the_selected_option_then_confirms() {
    let (mut app, mut writes) = app_with_dialog(CLAUDE_BASH_DIALOG, Some("session"));
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    assert!(app.agent_info(0, pane_id).unwrap().dialog_id.is_some());
    let observation = observe_dialog(&mut app);
    let dialog = observation.dialog.clone().unwrap();
    assert_eq!(dialog.text, "Do you want to proceed?");
    assert_eq!(dialog.options.len(), 3);
    assert!(dialog.options[0].selected);

    let choice = chosen(&choose(
        &mut app,
        &observation,
        Some("session"),
        &dialog.digest,
        3,
    ));
    assert!(choice.written);
    assert_eq!(choice.keys, ["down", "down", "enter"]);
    let moves = tokio::time::timeout(Duration::from_secs(2), writes.recv()).await;
    assert_eq!(moves.unwrap().unwrap(), Bytes::from_static(b"\x1b[B\x1b[B"));
    let enter = tokio::time::timeout(Duration::from_secs(2), writes.recv()).await;
    assert_eq!(enter.unwrap().unwrap(), Bytes::from_static(b"\r"));
}

#[tokio::test]
async fn agent_dialog_choose_finds_a_highlighted_selection_without_a_marker() {
    // The cursor row is drawn in inverse video; no marker character.
    let screen = b" Choose a model\r\n\r\n   1. Fast\r\n\x1b[7m   2. Balanced\x1b[0m\r\n   3. Thorough\r\n\r\n Enter to select \xc2\xb7 Esc to cancel\r\n";
    let (mut app, mut writes) = app_with_dialog(screen, Some("session"));
    let observation = observe_dialog(&mut app);
    let dialog = observation.dialog.clone().unwrap();
    let selected: Vec<_> = dialog
        .options
        .iter()
        .map(|option| option.selected)
        .collect();
    assert_eq!(selected, [false, true, false]);
    let choice = chosen(&choose(
        &mut app,
        &observation,
        Some("session"),
        &dialog.digest,
        1,
    ));
    assert_eq!(choice.keys, ["up", "enter"]);
    let moves = tokio::time::timeout(Duration::from_secs(2), writes.recv()).await;
    assert_eq!(moves.unwrap().unwrap(), Bytes::from_static(b"\x1b[A"));
    let enter = tokio::time::timeout(Duration::from_secs(2), writes.recv()).await;
    assert_eq!(enter.unwrap().unwrap(), Bytes::from_static(b"\r"));
}

#[tokio::test]
async fn agent_dialog_choose_works_while_launching_without_a_session() {
    let (mut app, mut writes) = app_with_dialog(CLAUDE_BASH_DIALOG, None);
    let observation = observe_dialog(&mut app);
    assert_eq!(observation.session_id, None);
    let digest = observation.dialog.clone().unwrap().digest;
    let choice = chosen(&choose(&mut app, &observation, None, &digest, 1));
    assert_eq!(choice.keys, ["enter"]);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\r"));
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn agent_dialog_choose_sends_nothing_for_stale_missing_or_foreign_dialogs() {
    let (mut app, mut writes) = app_with_dialog(CLAUDE_BASH_DIALOG, Some("session"));
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let observation = observe_dialog(&mut app);
    let digest = observation.dialog.clone().unwrap().digest;

    let error: crate::protocol::api::schema::ErrorResponse = serde_json::from_str(&choose(
        &mut app,
        &observation,
        Some("other-session"),
        &digest,
        1,
    ))
    .unwrap();
    assert_eq!(error.error.code, "agent_identity_changed");

    let missing = chosen(&choose(&mut app, &observation, Some("session"), &digest, 4));
    assert!(!missing.written);
    assert_eq!(
        missing.reason.as_deref(),
        Some("option_missing_or_selection_not_visible")
    );

    app.lookup_runtime_sender(0, pane_id)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[2J\x1b[H Allow rm -rf build?\r\n \xe2\x9d\xaf 1. Yes\r\n   2. No\r\n\r\n Esc to cancel\r\n");
    let stale = chosen(&choose(&mut app, &observation, Some("session"), &digest, 1));
    assert!(!stale.written);
    assert_eq!(stale.reason.as_deref(), Some("stale_or_changed_dialog"));
    assert!(stale.keys.is_empty());

    app.lookup_runtime_sender(0, pane_id)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[2J\x1b[H\xe2\x9d\xaf \r\n");
    let idle = observe_dialog(&mut app);
    assert_eq!(idle.dialog, None);
    assert_eq!(app.agent_info(0, pane_id).unwrap().dialog_id, None);
    assert!(!chosen(&choose(&mut app, &idle, Some("session"), &digest, 1)).written);
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_queued_question_observation_opens_the_fixture_panel() {
    let screen = include_str!("../../../../../tests/fixtures/codex-question/collapsed.txt");
    let (mut app, _) = app_with_dialog(b"", None);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_agent_name("reviewer".into());
    let (runtime, mut writes) =
        crate::terminal::runtime::TerminalRuntime::test_with_channel(131, 45);
    runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
    app.state.insert_test_runtime(pane, runtime);
    let request: crate::protocol::api::schema::Request =
        serde_json::from_value(serde_json::json!({
            "id": "open-question", "method": "agent.dialog.observe",
            "params": {"target": "reviewer", "open_pending_question": true}
        }))
        .unwrap();
    let crate::protocol::api::schema::Method::AgentDialogObserve(params) = request.method else {
        panic!("observe request");
    };
    assert!(crate::agents::dialog::codex_question_pending(screen));
    let response = app.handle_agent_dialog_observe(request.id, params);
    assert!(
        serde_json::from_str::<SuccessResponse>(&response).is_ok(),
        "{response}"
    );
    assert_eq!(
        writes
            .try_recv()
            .expect("observation must open the queued question"),
        Bytes::from_static(b"\x1b[1;2D")
    );
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_queued_question_native_controls_are_guarded_and_answer_focused_other() {
    let (mut app, _) = app_with_dialog(b"", None);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let (runtime, mut writes) =
        crate::terminal::runtime::TerminalRuntime::test_with_channel(131, 45);
    let collapsed = include_str!("../../../../../tests/fixtures/codex-question/collapsed.txt");
    runtime.test_process_pty_bytes(collapsed.replace('\n', "\r\n").as_bytes());
    app.state.insert_test_runtime(pane, runtime);
    let observe = |app: &mut App, open| {
        app.handle_agent_dialog_observe(
            "observe".into(),
            crate::protocol::api::schema::AgentDialogObserveParams {
                target: "reviewer".into(),
                open_pending_question: open,
            },
        )
    };
    // The same screen in a different provider must not receive Codex navigation.
    observe(&mut app, true);
    assert!(writes.try_recv().is_err());
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_agent_name("reviewer".into());
    let passive: SuccessResponse = serde_json::from_str(&observe(&mut app, false)).unwrap();
    let ResponseResult::AgentDialog {
        observation: passive,
    } = passive.result
    else {
        panic!("observation");
    };
    assert!(passive.pending_question);
    assert!(passive.dialog.is_none());
    assert!(
        app.agent_info(0, pane).unwrap().dialog_id.is_some(),
        "collapsed question must reach coordinator dialog polling"
    );
    assert!(writes.try_recv().is_err());
    let other = include_str!("../../../../../tests/fixtures/codex-question/other-selected.txt");
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", other.replace('\n', "\r\n")).as_bytes());
    let observation = observe_dialog(&mut app);
    assert_eq!(
        observation.dialog.as_ref().unwrap().kind,
        AgentDialogKind::Question
    );
    let params = answer_params(&observation, Some("fixture answer"), false);
    let choice = chosen(&app.handle_agent_dialog_answer("answer".into(), params));
    assert!(choice.written);
    assert_eq!(
        writes.recv().await.unwrap(),
        Bytes::from_static(b"fixture answer")
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"\r")
    );
    let typed = include_str!("../../../../../tests/fixtures/codex-question/other-typed.txt");
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", typed.replace('\n', "\r\n")).as_bytes());
    let stale = chosen(&app.handle_agent_dialog_answer(
        "stale".into(),
        answer_params(&observation, Some("do not send"), false),
    ));
    assert!(!stale.written);
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn dialog_observation_rejects_frames_inside_synchronized_output() {
    let (mut app, _) = app_with_dialog(CLAUDE_BASH_DIALOG, None);
    assert!(observe_dialog(&mut app).dialog.is_some());
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    // A provider starts an atomic redraw, clears its panel, then pauses
    // before drawing the replacement and ending the synchronized update.
    runtime.test_process_pty_bytes(b"\x1b[?2026h\x1b[2J\x1b[H");
    assert!(runtime.synchronized_output_active());
    let response = app.handle_agent_dialog_observe(
        "mid-update".into(),
        crate::protocol::api::schema::AgentDialogObserveParams {
            target: "reviewer".into(),
            open_pending_question: false,
        },
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        response["error"]["code"], "dialog_observation_unavailable",
        "{response}"
    );
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    runtime.test_process_pty_bytes(CLAUDE_BASH_DIALOG);
    runtime.test_process_pty_bytes(b"\x1b[?2026l");
    assert!(!runtime.synchronized_output_active());
    assert!(observe_dialog(&mut app).dialog.is_some());
}

#[tokio::test]
async fn agent_dialog_detection_preserves_raw_idle_status_for_poll() {
    let (mut app, _) = app_with_dialog(CLAUDE_BASH_DIALOG, Some("session"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    let info = app.agent_info(0, pane).unwrap();
    assert!(info.dialog_id.is_some());
    assert_eq!(
        info.agent_status,
        crate::protocol::api::schema::AgentStatus::Idle
    );
}

#[tokio::test]
async fn claude_dialog_actions_send_native_rows_and_arrows_and_reject_checkbox_edits() {
    let screen = include_str!("../../../../../tests/fixtures/claude-question/multi-select.txt")
        .replace('\n', "\r\n");
    let (mut app, mut writes) = app_with_dialog(screen.as_bytes(), Some("session"));
    for (option, moves) in [
        (6, b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B".as_slice()),
        (7, b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B".as_slice()),
    ] {
        let observed = observe_dialog(&mut app);
        let result = chosen(&choose(
            &mut app,
            &observed,
            Some("session"),
            &observed.dialog.as_ref().unwrap().digest,
            option,
        ));
        assert!(result.written);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), writes.recv())
                .await
                .unwrap()
                .unwrap(),
            Bytes::copy_from_slice(moves)
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), writes.recv())
                .await
                .unwrap()
                .unwrap(),
            Bytes::from_static(b"\r")
        );
    }
    for (option, arrow) in [(8, b"\x1b[D".as_slice()), (9, b"\x1b[C".as_slice())] {
        let observed = observe_dialog(&mut app);
        let result = chosen(&choose(
            &mut app,
            &observed,
            Some("session"),
            &observed.dialog.as_ref().unwrap().digest,
            option,
        ));
        assert!(result.written);
        assert_eq!(writes.try_recv().unwrap(), Bytes::copy_from_slice(arrow));
        assert!(writes.try_recv().is_err()); // Tab navigation must not confirm an answer.
    }
    for edit in [
        screen.replace("[ ] Build", "[✔] Build"),
        screen.replace("☐ Tier 2 OK?", "\x1b[7m☐ Tier 2 OK?\x1b[0m"),
    ] {
        let observed = observe_dialog(&mut app);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes(format!("\x1b[2J\x1b[H{edit}").as_bytes());
        let result = chosen(&choose(
            &mut app,
            &observed,
            Some("session"),
            &observed.dialog.as_ref().unwrap().digest,
            6,
        ));
        assert!(!result.written);
        assert_eq!(result.reason.as_deref(), Some("stale_or_changed_dialog"));
        assert!(writes.try_recv().is_err());
    }
}

const CLAUDE_TABBED_CUSTOM_ANSWER: &str = "←  ☐ Storage  ☐ Region  ✔ Submit  →\r\n\r\nWhich storage?\r\n\r\n  1. Redis\r\n  2. In memory\r\n❯ 3. Type something.\r\n     Next\r\n────────────────\r\n  4. Chat about this\r\n\r\nEnter to select · Tab/Arrow keys to navigate\r\nctrl+g to edit in Vim · Esc to cancel\r\n";

#[tokio::test]
async fn claude_tabbed_custom_answer_is_observed_as_an_option_free_text_question() {
    // The coordinator refuses `agent answer` whenever the observation carries
    // options ("This is a choice dialog; use agent choose").
    let (mut app, _) = app_with_dialog(CLAUDE_TABBED_CUSTOM_ANSWER.as_bytes(), Some("session"));
    let dialog = observe_dialog(&mut app).dialog.unwrap();
    assert_eq!(dialog.kind, AgentDialogKind::Question, "{dialog:?}");
    assert!(
        dialog.options.is_empty(),
        "free-text question observed with options: {:?}",
        dialog
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn claude_tabbed_custom_answer_writes_text_and_skip_with_a_wrapped_hint() {
    let (mut app, mut writes) =
        app_with_dialog(CLAUDE_TABBED_CUSTOM_ANSWER.as_bytes(), Some("session"));
    let observed = observe_dialog(&mut app);
    assert!(observed.dialog.as_ref().unwrap().options.is_empty());
    let answered = chosen(&app.handle_agent_dialog_answer(
        "answer".into(),
        answer_params(&observed, Some("Redis"), false),
    ));
    assert!(answered.written);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"Redis")
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"\r")
    );
    let skipped = chosen(
        &app.handle_agent_dialog_answer("skip".into(), answer_params(&observed, None, true)),
    );
    assert!(skipped.written);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\x1b"));
}

#[test]
fn claude_tabbed_custom_answer_identity_survives_typing() {
    let untouched = crate::agents::dialog::parse(CLAUDE_TABBED_CUSTOM_ANSWER).unwrap();
    let edited = crate::agents::dialog::parse(
        &CLAUDE_TABBED_CUSTOM_ANSWER.replace("Type something.", "MY_TOKEN"),
    )
    .unwrap();
    assert_ne!(untouched.digest(), edited.digest());
    assert_eq!(untouched.id(), edited.id());
}

#[test]
fn claude_checkbox_descriptions_keep_submit_before_chat() {
    let screen = "←  ☐ Caches  ✔ Submit  →\n\nWhich caches may be removed?\n\n❯ 1. [ ] Build cache\n       target and dist outputs\n  2. [ ] Package cache\n       registry downloads\n  3. [ ] Type something\n     Submit\n────────────────\n  4. Chat about this\n\nEnter to toggle · Tab/Arrow keys to navigate · Esc to cancel\n";
    let dialog = crate::agents::dialog::parse(screen).unwrap();
    let labels: Vec<_> = dialog
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    assert_eq!(
        labels[..5],
        [
            "[ ] Build cache target and dist outputs",
            "[ ] Package cache registry downloads",
            "[ ] Type something",
            "Submit",
            "Chat about this",
        ]
    );
    assert_eq!(
        dialog.keys_for(4).unwrap(),
        ["down", "down", "down", "enter"]
    );
}

#[test]
fn claude_review_screen_under_tab_header_offers_submit_and_cancel() {
    let screen = "←  ☒ Storage  ☒ Region  ✔ Submit  →\n\nReview your answers\n\n ● Which storage?\n   → Redis\n\nReady to submit your answers?\n\n❯ 1. Submit answers\n  2. Cancel\n\nEnter to select · Tab/Arrow keys to navigate · Esc to cancel\n";
    let dialog = crate::agents::dialog::parse(screen).unwrap();
    let labels: Vec<_> = dialog
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    assert_eq!(labels, ["Submit answers", "Cancel"]);
    assert_eq!(dialog.keys_for(1).unwrap(), ["enter"]);
}
