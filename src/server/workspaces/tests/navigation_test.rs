#[test]
fn switch_workspace_updates_active_and_selected() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.switch_workspace(2);
    assert_eq!(state.active, Some(2));
    assert_eq!(state.selected, 2);
}

#[test]
fn switch_workspace_marks_panes_seen() {
    let mut state = app_with_workspaces(&["a", "b"]);
    // Mark a pane in workspace 1 as unseen
    let id = *state.workspaces[1].panes.keys().next().unwrap();
    state.workspaces[1].panes.get_mut(&id).unwrap().seen = false;

    state.switch_workspace(1);
    assert!(state.workspaces[1].panes.get(&id).unwrap().seen);
}

#[test]
fn switch_workspace_out_of_bounds_is_noop() {
    let mut state = app_with_workspaces(&["a"]);
    state.switch_workspace(5);
    assert_eq!(state.active, Some(0));
}

#[test]
fn move_workspace_reorders_without_changing_logical_selection() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    let active_id = state.workspaces[1].id.clone();
    let selected_id = state.workspaces[2].id.clone();
    state.active = Some(1);
    state.selected = 2;

    state.move_workspace(1, 0);

    let names: Vec<_> = state
        .workspaces
        .iter()
        .map(|ws| ws.display_name())
        .collect();
    assert_eq!(names, vec!["b", "a", "c"]);
    assert_eq!(state.active, Some(0));
    assert_eq!(state.selected, 2);
    assert_eq!(state.workspaces[state.active.unwrap()].id, active_id);
    assert_eq!(state.workspaces[state.selected].id, selected_id);
}

#[test]
fn move_workspace_accepts_insert_at_end() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);

    state.move_workspace(0, state.workspaces.len());

    let names: Vec<_> = state
        .workspaces
        .iter()
        .map(|ws| ws.display_name())
        .collect();
    assert_eq!(names, vec!["b", "c", "a"]);
}

#[test]
fn pane_died_last_pane_removes_workspace() {
    let mut state = app_with_workspaces(&["a", "b"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_pane_died(pane_id);

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].custom_name.as_deref(), Some("b"));
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_last_workspace_enters_navigate() {
    let mut state = app_with_workspaces(&["only"]);
    state.mode = Mode::Terminal;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_pane_died(pane_id);

    assert!(state.workspaces.is_empty());
    assert_eq!(state.mode, Mode::Navigate);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_multi_pane_keeps_workspace() {
    let mut state = app_with_workspaces(&["test"]);
    let second_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    state.handle_pane_died(second_id);

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].panes.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_unknown_pane_is_noop() {
    let mut state = app_with_workspaces(&["test"]);
    let fake_id = PaneId::from_raw(9999);

    state.handle_pane_died(fake_id);

    assert_eq!(state.workspaces.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn state_changed_updates_pane() {
    let mut state = app_with_workspaces(&["test"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        exited_process: None, observed_at: std::time::Instant::now(),
    });

    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Working);
    assert_eq!(terminal.detected_agent, Some(AgentKind::Pi));
}

#[test]
fn terminal_cwd_report_updates_terminal_cwd_and_marks_session_dirty() {
    let mut state = app_with_workspaces(&["active"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    let cwd = std::env::temp_dir().join(format!("bus-cwd-report-test-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    state.session_dirty = false;

    let updates = state.handle_app_event(TerminalEvent::TerminalCwdReported {
        pane_id,
        cwd: cwd.clone(),
    });

    assert!(updates.is_empty());
    assert_eq!(state.terminals.get(&terminal_id).unwrap().cwd, cwd);
    assert!(state.session_dirty);
    let _ = std::fs::remove_dir_all(cwd);
}

#[test]
fn toggle_zoom_works() {
    let mut state = app_with_workspaces(&["test"]);
    state.workspaces[0].test_split(Direction::Horizontal);

    assert!(!state.workspaces[0].zoomed);
    state.toggle_zoom();
    assert!(state.workspaces[0].zoomed);
    state.toggle_zoom();
    assert!(!state.workspaces[0].zoomed);
}

#[test]
fn toggle_zoom_single_pane_noop() {
    let mut state = app_with_workspaces(&["test"]);
    state.toggle_zoom();
    assert!(!state.workspaces[0].zoomed);
}

#[test]
fn navigate_pane_changes_focus_while_zoomed() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);
    state.workspaces[0].layout.focus_pane(root);
    state.workspaces[0].zoomed = true;
    crate::server::rendering::surface::compute_view_with_runtime_registry(
        &mut state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );

    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, root);

    state.navigate_pane(NavDirection::Right);
    crate::server::rendering::surface::compute_view_with_runtime_registry(
        &mut state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );

    assert!(state.workspaces[0].zoomed);
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(right));
    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, right);
    assert!(state.view.pane_infos[0].inner_rect.x > state.view.pane_infos[0].rect.x);
}
