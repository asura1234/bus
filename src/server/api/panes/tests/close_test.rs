use super::*;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn guarded_close_preserves_a_released_shell_with_a_running_background_job() {
    let (mut app, mut params, pane_id, _) = guarded_close_fixture();
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    let (events, _event_rx) = tokio::sync::mpsc::channel(64);
    let runtime = crate::terminal::TerminalRuntime::spawn_argv_command(
        pane_id,
        24,
        80,
        std::env::temp_dir(),
        &["/bin/sh".into(), "-i".into()],
        &crate::terminal::runtime::PaneLaunchEnv::default(),
        crate::terminal::runtime::AgentDetection::Disabled,
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let shell_pid = runtime.child_pid().unwrap();
    runtime
        .try_send_bytes(bytes::Bytes::from_static(b"sleep 30 &\n"))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let processes = crate::platform::session_processes(shell_pid);
        if processes.iter().any(|pid| *pid != shell_pid)
            && crate::server::terminals::agents::available_shell_name(&runtime).is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background job did not start with shell in foreground"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.clear_agent_name();
    terminal.set_detected_state(None, AgentState::Idle);
    params.close_exited_shell = true;
    let response = app.handle_pane_close_if_identity("background".into(), params);
    assert!(
        app.state.workspaces[0].pane_state(pane_id).is_some(),
        "deletion must preserve a released shell running a background job; response: {response}"
    );
}

#[test]
fn guarded_close_rejects_changed_ownership_before_stopping_any_terminal() {
    let (mut app, params, pane_id, other) = guarded_close_fixture();
    let count = app.state.terminals.len();
    for field in ["pane", "terminal", "name", "provider", "session"] {
        let mut wrong = params.clone();
        match field {
            "pane" => wrong.pane_id = app.public_pane_id(0, other).unwrap(),
            "terminal" => {
                wrong.expected_terminal_id = app
                    .state
                    .terminal_id_for_pane(0, other)
                    .unwrap()
                    .to_string()
            }
            "name" => wrong.expected_managed_name = "different-launch".into(),
            "provider" => wrong.expected_agent = "claude".into(),
            "session" => wrong.expected_session_id = Some("rebound-session".into()),
            _ => unreachable!(),
        }
        let response = app.handle_pane_close_if_identity("guard".into(), wrong);
        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "terminal_identity_changed", "{field}");
        assert_eq!(app.state.terminals.len(), count);
        assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
        assert!(app.state.workspaces[0].pane_state(other).is_some());
    }
}

#[tokio::test]
async fn guarded_close_removes_only_exact_terminal_and_retries_absence_safely() {
    let (mut app, params, pane_id, other) = guarded_close_fixture();
    let other_terminal = app.state.terminal_id_for_pane(0, other).unwrap();
    let (runtime, _rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    app.terminal_runtimes
        .insert(app.state.terminal_id_for_pane(0, pane_id).unwrap(), runtime);
    let response = app.handle_pane_close_if_identity("close".into(), params.clone());
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(success.result, ResponseResult::Ok {}));
    assert!(app.state.workspaces[0].pane_state(pane_id).is_none());
    assert!(app.state.terminals.contains_key(&other_terminal));
    assert!(!app
        .terminal_runtimes
        .contains_id(&params.expected_terminal_id));
    // A different pane remains safe even if supplied in a retry after the
    // expected terminal was already stopped and removed.
    let mut retry = params;
    retry.pane_id = app.public_pane_id(0, other).unwrap();
    let response = app.handle_pane_close_if_identity("retry".into(), retry);
    assert!(serde_json::from_str::<SuccessResponse>(&response).is_ok());
    assert!(app.state.workspaces[0].pane_state(other).is_some());
}

/// Regression: Codex exited during bootstrap and its pane fell back to the
/// shell, which cleared the managed name; deleting the agent left that idle
/// Bus-created shell open forever.
#[tokio::test]
async fn guarded_close_of_an_exited_provider_closes_only_its_idle_released_shell() {
    let (mut app, params, pane_id, _) = guarded_close_fixture();
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    let (runtime, _rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let exited = crate::protocol::api::schema::PaneCloseIfIdentityParams {
        close_exited_shell: true,
        ..params.clone()
    };
    // While the provider still runs, the exited-shell form proves nothing.
    let response = app.handle_pane_close_if_identity("running".into(), exited.clone());
    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "terminal_identity_changed");
    assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.clear_agent_name();
    terminal.set_detected_state(None, AgentState::Idle);
    // The ordinary guard still refuses the released terminal.
    let response = app.handle_pane_close_if_identity("plain".into(), params);
    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "terminal_identity_changed");
    // Another managed name on the same terminal is not a released shell.
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_agent_name("bus-r1-a9".into());
    let response = app.handle_pane_close_if_identity("renamed".into(), exited.clone());
    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "terminal_identity_changed");
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .clear_agent_name();
    let response = app.handle_pane_close_if_identity("exited".into(), exited);
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(success.result, ResponseResult::Ok {}));
    assert!(app.state.workspaces[0].pane_state(pane_id).is_none());
}

#[test]
fn guarded_close_refuses_a_terminal_still_attached_to_multiple_panes() {
    let (mut app, params, pane_id, other) = guarded_close_fixture();
    let terminal = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    app.state.workspaces[0].tabs[0]
        .panes
        .get_mut(&other)
        .unwrap()
        .attached_terminal_id = terminal;
    let response = app.handle_pane_close_if_identity("shared".into(), params);
    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "terminal_identity_changed");
    assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
    assert!(app.state.workspaces[0].pane_state(other).is_some());
}

#[test]
fn api_pane_close_of_last_pane_closes_its_workspace() {
    let mut app = app_with_one_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

    let response = app.handle_pane_close(
        "req".into(),
        PaneTarget {
            pane_id: public_pane_id,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "req");
    assert!(app.state.workspaces.is_empty());
}

/// The exited-shell close must still close a real shell that sits idle at its
/// prompt, not only the pid-less runtime of the other close tests.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn guarded_close_of_an_exited_provider_closes_a_real_idle_shell() {
    let (mut app, mut params, pane_id, _) = guarded_close_fixture();
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    let (events, _event_rx) = tokio::sync::mpsc::channel(64);
    let runtime = crate::terminal::TerminalRuntime::spawn_argv_command(
        pane_id,
        24,
        80,
        std::env::temp_dir(),
        &["/bin/sh".into(), "-i".into()],
        &crate::terminal::runtime::PaneLaunchEnv::default(),
        crate::terminal::runtime::AgentDetection::Disabled,
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while crate::server::terminals::agents::available_shell_name(&runtime).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "shell never reached its prompt"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.clear_agent_name();
    terminal.set_detected_state(None, AgentState::Idle);
    params.close_exited_shell = true;
    let response = app.handle_pane_close_if_identity("idle".into(), params);
    assert!(
        app.state.workspaces[0].pane_state(pane_id).is_none(),
        "an idle released shell must be closed; response: {response}"
    );
}

/// A provider Bus started exited on its own and its shell now runs something
/// else (an updater prompt, a command). With `allow_busy_shell` that
/// Bus-created terminal is closed anyway, but only on the server's own exit
/// evidence: a busy released shell without it is still preserved.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn guarded_close_of_an_exited_provider_closes_its_busy_shell_only_on_exit_evidence() {
    let (mut app, mut params, pane_id, _) = guarded_close_fixture();
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    let (events, _event_rx) = tokio::sync::mpsc::channel(64);
    let runtime = crate::terminal::TerminalRuntime::spawn_argv_command(
        pane_id,
        24,
        80,
        std::env::temp_dir(),
        &["/bin/sh".into(), "-i".into()],
        &crate::terminal::runtime::PaneLaunchEnv::default(),
        crate::terminal::runtime::AgentDetection::Disabled,
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let shell_pid = runtime.child_pid().unwrap();
    runtime
        .try_send_bytes(bytes::Bytes::from_static(b"sleep 30\n"))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !crate::platform::session_processes(shell_pid)
        .iter()
        .any(|pid| *pid != shell_pid)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "foreground command did not start"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.clear_agent_name();
    terminal.set_detected_state(None, AgentState::Idle);
    params.close_exited_shell = true;
    params.allow_busy_shell = true;
    let response = app.handle_pane_close_if_identity("no-evidence".into(), params.clone());
    assert!(
        app.state.workspaces[0].pane_state(pane_id).is_some(),
        "a busy shell without exit evidence must be preserved; response: {response}"
    );
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_detected_state_with_visible_blocker(
        Some(crate::agents::AgentKind::Codex),
        AgentState::Idle,
        false,
        true,
    );
    terminal.set_detected_state(None, AgentState::Idle);
    let response = app.handle_pane_close_if_identity("exited".into(), params);
    assert!(
        app.state.workspaces[0].pane_state(pane_id).is_none(),
        "the exited provider's busy shell must be closed; response: {response}"
    );
}
