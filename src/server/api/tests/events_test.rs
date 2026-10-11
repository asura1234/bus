use super::*;

#[test]
fn pane_exit_emits_layout_updated_when_tab_survives() {
    let event_hub = crate::server::api::EventHub::default();
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        event_hub.clone(),
    );
    let mut workspace = crate::server::workspaces::Workspace::test_new("pane-exit-layout");
    let dead_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let tab_id = app.public_tab_id(0, 0).unwrap();

    app.handle_internal_event(TerminalEvent::PaneDied {
        pane_id: dead_pane,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    let events = event_hub.events_after(0);
    let pane_exited = events
        .iter()
        .position(|(_, event)| event.event == crate::protocol::api::schema::EventKind::PaneExited)
        .expect("pane.exited should be emitted");
    let layout_updated = events
        .iter()
        .position(|(_, event)| {
            event.event == crate::protocol::api::schema::EventKind::LayoutUpdated
        })
        .expect("layout.updated should be emitted");
    assert!(pane_exited < layout_updated);
    assert!(matches!(
        &events[layout_updated].1.data,
        crate::protocol::api::schema::EventData::LayoutUpdated { layout }
            if layout.tab_id == tab_id && layout.panes.len() == 1
    ));
}

#[test]
fn idle_agent_exit_emits_release_event_without_a_state_change() {
    for agent_name in [None, Some("reviewer")] {
        let event_hub = crate::server::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::utils::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            event_hub.clone(),
        );
        let workspace = crate::server::workspaces::Workspace::test_new("idle-agent-exit");
        let pane_id = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(AgentKind::Pi), AgentState::Idle);
        if let Some(agent_name) = agent_name {
            terminal.set_agent_name(agent_name.into());
        }

        app.handle_internal_event(TerminalEvent::StateChanged {
            pane_id,
            agent: Some(AgentKind::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: true,
            exited_process: None,
            observed_at: std::time::Instant::now(),
        });

        assert!(app.state.terminals[&terminal_id].agent_name.is_none());
        assert!(event_hub.events_after(0).iter().any(|(_, event)| matches!(
            &event.data,
            crate::protocol::api::schema::EventData::PaneAgentDetected {
                released: true,
                final_status: Some(crate::protocol::api::schema::AgentStatus::Idle),
                ..
            }
        )));
    }
}

#[test]
fn overlay_exit_layout_updated_uses_restored_zoom_state() {
    let event_hub = crate::server::api::EventHub::default();
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        event_hub.clone(),
    );
    let mut workspace = crate::server::workspaces::Workspace::test_new("overlay-layout");
    let previous_focus = workspace.tabs[0].root_pane;
    let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(previous_focus);
    workspace.tabs[0].zoomed = true;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.mode = Mode::Terminal;
    let tab_id = app.public_tab_id(0, 0).unwrap();
    app.overlay_panes.insert(
        overlay_pane,
        OverlayPaneState {
            ws_idx: 0,
            tab_idx: 0,
            previous_focus,
            previous_zoomed: false,
            temp_files: Vec::new(),
        },
    );

    app.handle_internal_event(TerminalEvent::PaneDied {
        pane_id: overlay_pane,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    let events = event_hub.events_after(0);
    let layout_updated = events
        .iter()
        .rposition(|(_, event)| {
            event.event == crate::protocol::api::schema::EventKind::LayoutUpdated
        })
        .expect("layout.updated should be emitted");
    assert!(matches!(
        &events[layout_updated].1.data,
        crate::protocol::api::schema::EventData::LayoutUpdated { layout }
            if layout.tab_id == tab_id && layout.zoomed
    ));
}

#[test]
fn terminal_delivery_does_not_refresh_existing_targeted_toast() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    let mut workspace = crate::server::workspaces::Workspace::test_new("stale");
    workspace.custom_name = None;
    workspace.identity_cwd = "/__bus_original__".into();
    let root = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(root).cloned().unwrap();
    let workspace_id = workspace.id.clone();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.terminals.get_mut(&terminal_id).unwrap().cwd = "/__bus_projects__".into();
    app.state.active = None;
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    app.state.toast_config.delivery = crate::utils::config::ToastDelivery::Terminal;

    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id: root,
        agent: Some(AgentKind::Codex),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });
    app.state.toast = Some(crate::server::app_state::ToastNotification {
        kind: ToastKind::Finished,
        title: "codex finished".into(),
        context: "__bus_original__ · 1".into(),
        position: None,
        target: Some(crate::server::app_state::ToastTarget {
            workspace_id,
            pane_id: root,
        }),
    });

    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id: root,
        agent: Some(AgentKind::Codex),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });

    assert_eq!(
        app.state.toast.as_ref().map(|toast| toast.context.as_str()),
        Some("__bus_original__ · 1")
    );
}
