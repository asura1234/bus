use super::*;

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
