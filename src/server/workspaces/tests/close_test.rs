#[test]
fn close_workspace_adjusts_indices() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.selected = 1;
    state.active = Some(1);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.selected, 1);
    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].custom_name.as_deref(), Some("c"));
}

#[test]
fn close_last_workspace_clears_active() {
    let mut state = app_with_workspaces(&["only"]);
    state.selected = 0;
    state.close_selected_workspace();

    assert!(state.workspaces.is_empty());
    assert_eq!(state.active, None);
    assert_eq!(state.selected, 0);
}

#[test]
fn close_workspace_at_end_adjusts_selected() {
    let mut state = app_with_workspaces(&["a", "b"]);
    state.selected = 1;
    state.active = Some(1);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.selected, 0);
    assert_eq!(state.active, Some(0));
}

#[test]
fn close_non_focused_workspace_keeps_focus() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.selected = 1;
    state.active = Some(0);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.workspaces[0].display_name(), "a");
    assert_eq!(state.workspaces[1].display_name(), "c");
    assert_eq!(state.selected, 0);
    assert_eq!(state.active, Some(0));
    state.assert_invariants_for_test();
}

#[test]
fn delayed_background_waiting_is_cleared_when_pane_dies() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Bus;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    state.handle_app_event(TerminalEvent::PaneDied {
        pane_id: bg_pane_id,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    assert!(state.pending_agent_notifications.is_empty());
    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn close_pane_removes_from_workspace() {
    let mut state = app_with_workspaces(&["test"]);
    state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();
    assert_eq!(state.workspaces[0].panes.len(), 2);
    state.close_pane();
    assert_eq!(state.workspaces[0].panes.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn pane_process_exit_publish_marks_agent_idle_before_pane_removal() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Bus;
    state.active = Some(1);
    state.ensure_test_terminals();
    let pane_id = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Pi), AgentState::Working);
    assert_eq!(
        state.terminals.get(&terminal_id).unwrap().state,
        AgentState::Working
    );

    let update = state
        .publish_pane_process_exit_if_agent(pane_id, false)
        .expect("process exit update");

    assert!(!state.pane_is_in_active_tab(update.ws_idx, pane_id));
    assert_eq!(update.previous_state, AgentState::Working);
    assert_eq!(update.state, AgentState::Idle);
    assert_eq!(update.agent_label.as_deref(), Some("pi"));
    assert_eq!(update.known_agent, Some(AgentKind::Pi));
    assert!(update.agent_released);
    assert_eq!(
        update.agent_release_status,
        Some(crate::protocol::api::schema::AgentStatus::Done)
    );
    assert!(matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));
}

#[test]
fn close_pane_removes_unattached_terminal_state() {
    let mut state = app_with_workspaces(&["test"]);
    let pane_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

    state.close_pane();

    assert!(!state.terminals.contains_key(&terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_workspace_removes_unattached_terminal_states() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let pane_id = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state.close_selected_workspace();

    assert!(!state.terminals.contains_key(&terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_pane_last_pane_closes_active_workspace_not_selected_workspace() {
    let mut state = app_with_workspaces(&["selected", "active"]);
    let active_terminal_id = state
        .terminal_id_for_pane(1, state.workspaces[1].tabs[0].root_pane)
        .unwrap();
    state.active = Some(1);
    state.selected = 0;

    state.close_pane();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "selected");
    assert!(!state.terminals.contains_key(&active_terminal_id));
    state.assert_invariants_for_test();
}
