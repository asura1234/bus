#[test]
fn notification_context_formats_resolved_workspace_label() {
    let state = app_with_workspaces(&["stale"]);
    let root = state.workspaces[0].tabs[0].root_pane;

    assert_eq!(
        notification_context(&state.workspaces[0], "__herdr_projects__", 0, root),
        "__herdr_projects__ · 1"
    );
}

#[test]
fn state_changed_idle_in_background_marks_unseen() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    // First set it to Working
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Working;

    // Now transition to Idle while in background
    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(!pane.seen);
    assert!(matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));
}

#[test]
fn active_tab_completion_marks_pane_seen() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(true);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&terminal_id).unwrap().state = AgentState::Working;
    state.workspaces[0].panes.get_mut(&pane_id).unwrap().seen = false;

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Idle);
    let pane = state.workspaces[0].panes.get(&pane_id).unwrap();
    assert!(pane.seen);
}

#[test]
fn initial_idle_in_background_stays_seen() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(pane.seen);
}

#[test]
fn idle_after_known_unknown_agent_in_background_marks_done() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Unknown,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(!pane.seen);
}

#[test]
fn first_idle_after_process_detection_is_not_completion() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::AgentProcessDetected {
        pane_id,
        agent: AgentKind::Pi,
        observed_at: Instant::now(),
    });
    let direct_idle = state
        .handle_app_event(TerminalEvent::StateChanged {
            pane_id,
            agent: Some(AgentKind::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: Instant::now(),
        })
        .pop()
        .expect("direct idle state update");
    assert!(direct_idle.suppress_completion);

    state.handle_app_event(TerminalEvent::AgentProcessDetected {
        pane_id,
        agent: AgentKind::Pi,
        observed_at: Instant::now(),
    });
    for agent_state in [AgentState::Working, AgentState::Blocked] {
        state.handle_app_event(TerminalEvent::StateChanged {
            pane_id,
            agent: Some(AgentKind::Pi),
            state: agent_state,
            visible_blocker: agent_state == AgentState::Blocked,
            process_exited: false,
            observed_at: Instant::now(),
        });
    }
    let update = state
        .handle_app_event(TerminalEvent::StateChanged {
            pane_id,
            agent: Some(AgentKind::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: Instant::now(),
        })
        .pop()
        .expect("idle state update");

    assert!(update.suppress_completion);
    assert!(state.workspaces[1].panes[&pane_id].seen);
    assert!(!matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));

    state.handle_app_event(TerminalEvent::AgentProcessDetected {
        pane_id,
        agent: AgentKind::Codex,
        observed_at: Instant::now(),
    });
    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(AgentKind::Codex),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        observed_at: Instant::now(),
    });
    let exit_update = state
        .handle_app_event(TerminalEvent::StateChanged {
            pane_id,
            agent: Some(AgentKind::Codex),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: true,
            observed_at: Instant::now(),
        })
        .pop()
        .expect("process exit update");
    assert!(!exit_update.suppress_completion);
}

#[test]
fn background_waiting_sets_attention_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2");
}

#[test]
fn delayed_background_waiting_schedules_before_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
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

    assert!(state.toast.is_none());
    assert!(state.pending_agent_notifications.contains_key(&bg_pane_id));

    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    let deliveries = state.drain_due_agent_notifications(deadline);
    assert_eq!(deliveries.len(), 1);

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2");
    assert!(state.pending_agent_notifications.is_empty());
}

#[test]
fn delayed_background_waiting_cancels_when_agent_resumes_working() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
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

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.pending_agent_notifications.is_empty());
    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_background_waiting_is_suppressed_if_pane_becomes_active() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
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
    state.active = Some(1);

    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_active_tab_unfocused_keeps_client_notification_available() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(false);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::System;
    state.toast_config.delay_seconds = 1;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    let deliveries = state.drain_due_agent_notifications(deadline);

    assert_eq!(deliveries.len(), 1);
    assert!(deliveries[0].toast.is_none());
    assert!(deliveries[0].client_notification.is_some());
    assert!(state.toast.is_none());
}

#[test]
fn background_idle_sets_finished_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Working;

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Droid),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::Finished);
    assert_eq!(toast.title, "droid finished");
    assert_eq!(toast.context, "background · 2");
    let target = toast.target.as_ref().expect("toast target");
    assert_eq!(&target.workspace_id, &state.workspaces[1].id);
    assert_eq!(target.pane_id, bg_pane_id);
}

#[test]
fn background_toast_includes_tab_name_when_workspace_has_multiple_tabs() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    state.workspaces[1].tabs[0].set_custom_name("main".into());
    let second_tab = state.workspaces[1].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    let bg_pane_id = state.workspaces[1].tabs[second_tab].root_pane;

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2 · logs");
}

#[test]
fn background_tab_in_active_workspace_still_sets_toast() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    state.workspaces[0].tabs[0].set_custom_name("main".into());
    let second_tab = state.workspaces[0].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    let bg_pane_id = state.workspaces[0].tabs[second_tab].root_pane;

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "active · 1 · logs");
}

#[test]
fn active_workspace_active_tab_does_not_set_toast() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
}

#[test]
fn active_workspace_active_tab_keeps_herdr_toast_suppressed_when_outer_terminal_is_unfocused() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(false);
    state.toast_config.delivery = crate::utils::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(AgentKind::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
}

#[test]
fn active_tab_suppression_preserves_unknown_focus_behavior() {
    assert!(active_tab_suppresses_notifications(true, None));
    assert!(active_tab_suppresses_notifications(true, Some(true)));
    assert!(!active_tab_suppresses_notifications(true, Some(false)));
    assert!(!active_tab_suppresses_notifications(false, None));
}
