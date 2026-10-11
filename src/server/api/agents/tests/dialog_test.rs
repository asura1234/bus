use super::*;

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
