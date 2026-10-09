use super::*;

#[test]
fn prompt_delay_only_scales_for_windows_codex() {
    let codex_delay = agent_prompt_submit_delay(AgentKind::Codex, 4_096);
    #[cfg(windows)]
    assert_eq!(codex_delay, Duration::from_millis(1_624));
    #[cfg(not(windows))]
    assert_eq!(codex_delay, AGENT_PROMPT_SUBMIT_DELAY);
    assert_eq!(
        agent_prompt_submit_delay(AgentKind::OpenCode, 4_096),
        AGENT_PROMPT_SUBMIT_DELAY
    );
}

#[test]
fn codex_composer_check_accepts_only_empty_prompt_states() {
    for screen in [
        "history\n›\n  gpt-5.6-sol",
        "history\n› Ask Codex to do anything\n  gpt-5.6-sol",
        "history\n› Use /skills to list available skills\n  gpt-5.6-sol",
    ] {
        assert!(codex_composer_is_empty(screen), "{screen}");
    }
    for screen in [
        "",
        "history without a prompt",
        "history\n› draft already present\n  gpt-5.6-sol",
        "history\n› check the logs\n  hello?\n  gpt-5.6-sol",
    ] {
        assert!(!codex_composer_is_empty(screen), "{screen}");
    }
}

#[test]
fn claude_input_check_ignores_suggestions_and_finds_typed_text() {
    let rule = "─".repeat(20);
    let screen = |input: &str| format!("● done\n\n{rule}\n{input}\n{rule}\n  ⏵⏵ auto mode on");
    for empty in [
        screen("❯\u{a0}"),
        // Claude's dimmed prompt suggestion, as captured from a live pane.
        screen("❯\u{a0}\x1b[0m\x1b[2madd the just linux-lint recipe\x1b[0m"),
        // The cursor cell drawn inverse over the suggestion's first letter.
        screen("❯ \x1b[7ma\x1b[0m\x1b[2mdd the recipe\x1b[0m"),
        // An earlier prompt in the history above the box is not the box.
        format!("❯ ok is it merged?\n\n{rule}\n❯\u{a0}\n{rule}"),
    ] {
        assert!(claude_input_is_empty(&empty), "{empty:?}");
    }
    for typed in [
        screen("❯\u{a0}master is where I coordinate"),
        // A 24-bit color's "2" is not dim.
        screen("❯ \x1b[38;2;200;200;200mtyped\x1b[0m"),
        // A wrapped second line holds the typed text.
        format!("{rule}\n❯\u{a0}\n  second line\n{rule}"),
        // No visible input box: Bus cannot tell, so it waits.
        "● working".to_owned(),
    ] {
        assert!(!claude_input_is_empty(&typed), "{typed:?}");
    }
}

#[tokio::test]
async fn guarded_claude_prompt_waits_while_the_input_box_holds_typed_text() {
    let (mut app, params, mut writes) =
        claude_agent_with_input("❯\u{a0}master is where I coordinate");
    let response = prompt_if_idle(&mut app, params);
    // agent_not_ready is a definite rejection: the Bus message stays queued.
    assert!(response.contains("agent_not_ready"), "{response}");
    assert!(response.contains("input box is not empty"), "{response}");
    assert!(writes.try_recv().is_err(), "nothing may be typed");
}

#[tokio::test]
async fn guarded_claude_prompt_types_into_an_empty_input_box() {
    for input in [
        "❯\u{a0}",
        "❯\u{a0}\x1b[2madd the just linux-lint recipe\x1b[0m",
    ] {
        let (mut app, params, mut writes) = claude_agent_with_input(input);
        let response = prompt_if_idle(&mut app, params);
        assert!(response.contains("agent_prompted"), "{input:?}: {response}");
        assert_eq!(
            writes.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
        );
        assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\r"));
    }
}

#[tokio::test]
async fn unbound_codex_prompt_requires_exact_ready_managed_launch_before_writing() {
    use crate::protocol::api::schema::{AgentPromptIfUnboundParams, Method, Request};
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let now = std::time::Instant::now();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        AgentKind::Codex,
        now,
        Duration::ZERO,
        Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
    let (runtime, mut rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    app.state.insert_test_runtime(pane_id, runtime);
    let info = app.agent_info(0, pane_id).unwrap();
    let params = AgentPromptIfUnboundParams {
        target: info.pane_id.clone(),
        text: "first room prompt".into(),
        expected_terminal_id: info.terminal_id.clone(),
        expected_pane_id: info.pane_id.clone(),
        expected_managed_name: "bus-r1-a2".into(),
    };
    let run = |app: &mut App, params: AgentPromptIfUnboundParams| {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_agent_api_request(
            Request {
                id: "bootstrap".into(),
                method: Method::AgentPromptIfUnbound(params)
            },
            tx
        ));
        rx.recv_timeout(Duration::from_secs(2)).unwrap()
    };
    for name in ["", "different-launch"] {
        let mut wrong = params.clone();
        wrong.expected_managed_name = name.into();
        assert!(run(&mut app, wrong).contains("agent_identity_changed"));
        assert!(rx.try_recv().is_err());
    }
    for state in [AgentState::Working, AgentState::Blocked] {
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Codex), state);
        assert!(run(&mut app, params.clone()).contains("agent_not_idle"));
        assert!(rx.try_recv().is_err());
    }
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    let mut wrong = info.clone();
    wrong.agent = Some("claude".into());
    assert!(check_unbound_prompt_identity_and_idle(&wrong, &params).is_err());
    let mut wrong = info.clone();
    wrong.interactive_ready = false;
    assert!(check_unbound_prompt_identity_and_idle(&wrong, &params).is_err());
    let mut wrong = info.clone();
    wrong.launch_pending = true;
    assert!(check_unbound_prompt_identity_and_idle(&wrong, &params).is_err());
    let mut wrong = params.clone();
    wrong.expected_terminal_id = "different".into();
    assert!(run(&mut app, wrong).contains("agent_identity_changed"));
    let mut wrong = params.clone();
    wrong.expected_pane_id = "different".into();
    assert!(run(&mut app, wrong).contains("agent_identity_changed"));
    assert!(rx.try_recv().is_err());
    assert!(run(&mut app, params.clone()).contains("agent_prompted"));
    assert_eq!(
        rx.try_recv().unwrap(),
        Bytes::from_static(b"\x1b[200~first room prompt\x1b[201~")
    );
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_agent_session_ref_for_session_start(
            "herdr:codex".into(),
            "codex".into(),
            crate::agents::resume::catalog::AgentSessionRef::id("already-bound"),
            Some(1),
            None,
        );
    assert!(run(&mut app, params).contains("agent_identity_changed"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn guarded_prompt_rechecks_identity_and_idle_then_reuses_delayed_enter() {
    use crate::protocol::api::schema::{AgentPromptIfIdleParams, Method, Request};
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    let now = std::time::Instant::now();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        AgentKind::Codex,
        now,
        Duration::ZERO,
        Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
    terminal.set_agent_session_ref_for_session_start(
        "bus".into(),
        "codex".into(),
        crate::agents::resume::catalog::AgentSessionRef::id("session"),
        Some(1),
        None,
    );
    let (runtime, mut rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    runtime.test_process_pty_bytes("\x1b[?2004h\x1b[2J\x1b[H› Ask Codex to do anything".as_bytes());
    app.state.insert_test_runtime(pane_id, runtime);
    let info = app.agent_info(0, pane_id).unwrap();
    let params = AgentPromptIfIdleParams {
        target: info.pane_id.clone(),
        text: "literal @x $HOME".into(),
        expected_terminal_id: info.terminal_id.clone(),
        expected_pane_id: info.pane_id.clone(),
        expected_agent: "codex".into(),
        expected_session_id: "session".into(),
        steer: false,
    };
    let run = |app: &mut App, params: AgentPromptIfIdleParams| {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_agent_api_request(
            Request {
                id: "guard".into(),
                method: Method::AgentPromptIfIdle(params)
            },
            tx
        ));
        rx
    };
    let mut wrong = params.clone();
    wrong.expected_session_id = "other".into();
    let rejected = run(&mut app, wrong)
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(rejected.contains("agent_identity_changed"), "{rejected}");
    assert!(rx.try_recv().is_err());
    for state in [AgentState::Working, AgentState::Blocked] {
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Codex), state);
        let rejected = run(&mut app, params.clone())
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(rejected.contains("agent_not_idle"), "{rejected}");
        assert!(rx.try_recv().is_err());
    }
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    let response = run(&mut app, params);
    assert!(response.try_recv().is_err());
    let result = response.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(result.contains("agent_prompted"), "{result}");
    assert_eq!(
        rx.try_recv().unwrap(),
        Bytes::from_static(b"\x1b[200~literal @x $HOME\x1b[201~")
    );
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
}

#[tokio::test]
async fn guarded_codex_prompt_rejects_nonempty_composer_without_writing() {
    use crate::protocol::api::schema::{AgentPromptIfIdleParams, Method, Request};

    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    let now = std::time::Instant::now();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        AgentKind::Codex,
        now,
        Duration::ZERO,
        Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
    terminal.set_agent_session_ref_for_session_start(
        "bus".into(),
        "codex".into(),
        crate::agents::resume::catalog::AgentSessionRef::id("session"),
        Some(1),
        None,
    );
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    runtime.test_process_pty_bytes("\x1b[?2004h\x1b[2J\x1b[H› draft already present".as_bytes());
    app.state.insert_test_runtime(pane_id, runtime);
    let info = app.agent_info(0, pane_id).unwrap();
    let (respond_to, response_rx) = std::sync::mpsc::channel();

    assert!(app.handle_deferred_agent_api_request(
        Request {
            id: "guard-nonempty".into(),
            method: Method::AgentPromptIfIdle(AgentPromptIfIdleParams {
                target: info.pane_id.clone(),
                text: "hello?".into(),
                expected_terminal_id: info.terminal_id,
                expected_pane_id: info.pane_id,
                expected_agent: "codex".into(),
                expected_session_id: "session".into(),
                steer: false,
            }),
        },
        respond_to,
    ));

    let response = response_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(response.contains("agent_not_ready"), "{response}");
    assert!(response.contains("composer is not empty"), "{response}");
    assert!(
        writes.try_recv().is_err(),
        "prompt bytes must not be written"
    );
}

#[tokio::test]
async fn agent_prompt_sends_text_then_delays_enter() {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_agent_name("reviewer".into());
    terminal.set_detected_state(Some(AgentKind::OpenCode), AgentState::Working);
    let (runtime, mut rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    app.state.insert_test_runtime(pane_id, runtime);

    let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
    let bracketed_started = std::time::Instant::now();
    let response_rx = start_deferred_agent_prompt(
        &mut app,
        "req",
        AgentPromptParams {
            target: public_pane_id,
            text: "A != B".into(),
            wait: None,
        },
    );
    assert!(response_rx.try_recv().is_err());
    let response = response_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("agent prompt responds after submission");
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::AgentPrompted { agent, .. } = success.result else {
        panic!("expected prompted response");
    };
    assert_eq!(agent.name.as_deref(), Some("reviewer"));
    assert_eq!(
        rx.try_recv().unwrap(),
        Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
    );
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    assert!(bracketed_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

    app.lookup_runtime_sender(0, pane_id)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[?2004l");
    let raw_started = std::time::Instant::now();
    let raw = run_deferred_agent_prompt(
        &mut app,
        "req-raw",
        AgentPromptParams {
            target: "reviewer".into(),
            text: "A != B".into(),
            wait: None,
        },
    );
    let raw: SuccessResponse = serde_json::from_str(&raw).unwrap();
    assert!(matches!(raw.result, ResponseResult::AgentPrompted { .. }));
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"A != B"));
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    assert!(raw_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

    let rejected = run_deferred_agent_prompt(
        &mut app,
        "req-label",
        AgentPromptParams {
            target: "opencode".into(),
            text: "wrong target".into(),
            wait: None,
        },
    );
    let error: crate::protocol::api::schema::ErrorResponse =
        serde_json::from_str(&rejected).unwrap();
    assert_eq!(error.error.code, "agent_not_found");
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn agent_prompt_rejects_blocked_agent_without_writing() {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_agent_name("reviewer".into());
    terminal.set_detected_state(Some(AgentKind::GithubCopilot), AgentState::Blocked);
    let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
    app.state.insert_test_runtime(pane_id, runtime);

    let response = run_deferred_agent_prompt(
        &mut app,
        "req",
        AgentPromptParams {
            target: "reviewer".into(),
            text: "unrelated prompt".into(),
            wait: None,
        },
    );

    let error: crate::protocol::api::schema::ErrorResponse =
        serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "agent_blocked");
    assert!(
        tokio::time::timeout(
            AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(100),
            rx.recv()
        )
        .await
        .is_err(),
        "blocked prompt wrote or scheduled terminal input"
    );
}

#[tokio::test]
async fn agent_prompt_focuses_copilot_before_submitting() {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_agent_name("reviewer".into());
    terminal.set_detected_state(Some(AgentKind::GithubCopilot), AgentState::Idle);
    let (runtime, mut rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 3);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    app.state.insert_test_runtime(pane_id, runtime);

    let response = run_deferred_agent_prompt(
        &mut app,
        "req",
        AgentPromptParams {
            target: "reviewer".into(),
            text: "A != B".into(),
            wait: None,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(
        success.result,
        ResponseResult::AgentPrompted { .. }
    ));
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[I"));
    assert_eq!(
        rx.try_recv().unwrap(),
        Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
    );
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
}

#[tokio::test]
async fn agent_prompt_rejects_managed_agent_while_startup_is_pending() {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    let now = std::time::Instant::now();
    terminal.begin_managed_agent(
        "reviewer".into(),
        AgentKind::OpenCode,
        now,
        std::time::Duration::from_secs(3),
        std::time::Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(AgentKind::OpenCode), AgentState::Idle);
    let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
    app.state.insert_test_runtime(pane_id, runtime);

    let response = run_deferred_agent_prompt(
        &mut app,
        "req-pending",
        AgentPromptParams {
            target: "reviewer".into(),
            text: "A != B".into(),
            wait: None,
        },
    );
    let error: crate::protocol::api::schema::ErrorResponse =
        serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "agent_not_ready");
    assert!(rx.try_recv().is_err());
}
