#[test]
fn terminal_bell_targets_foreground_client_only() {
    let mut server = test_headless_server();
    let (background_tx, background_control_rx, _background_rx) = test_client_writer();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            background_tx,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            2,
            foreground_tx,
        ),
    );
    server.foreground_client_id = Some(2);

    let changed = server.handle_internal_event_with_forwarding(TerminalEvent::TerminalBell {
        pane_id: crate::utils::ids::PaneId::from_raw(1),
        count: 3,
    });

    assert!(!changed);
    match read_server_message(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("foreground terminal bell message"),
    ) {
        ServerMessage::TerminalBell { count } => assert_eq!(count, 3),
        other => panic!("expected terminal bell message, got {other:?}"),
    }
    assert!(
        background_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "background client should not receive terminal bells"
    );

    server.foreground_client_id = None;
    server.handle_internal_event_with_forwarding(TerminalEvent::TerminalBell {
        pane_id: crate::utils::ids::PaneId::from_raw(1),
        count: 1,
    });
    assert!(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "bells without a foreground client must not be retained"
    );
}

#[test]
fn clipboard_write_targets_foreground_client_only() {
    let mut server = test_headless_server();
    let (background_tx, background_control_rx, _background_rx) = test_client_writer();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            background_tx,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            2,
            foreground_tx,
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    let changed = server.handle_internal_event_with_forwarding(TerminalEvent::ClipboardWrite {
        content: b"test".to_vec(),
    });

    assert!(!changed);
    match read_server_message(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("foreground clipboard message"),
    ) {
        ServerMessage::Clipboard { data } => assert_eq!(data, "dGVzdA=="),
        other => panic!("expected clipboard message, got {other:?}"),
    }
    assert!(
        background_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "background client should not receive clipboard writes"
    );
}

#[test]
fn clipboard_write_without_foreground_client_does_not_change_visual_state() {
    let mut server = test_headless_server();
    server.foreground_client_id = None;

    let changed = server.handle_internal_event_with_forwarding(TerminalEvent::ClipboardWrite {
        content: b"test".to_vec(),
    });

    assert!(!changed);
}

#[test]
fn clipboard_write_failed_foreground_send_removes_client_without_visual_change() {
    let mut server = test_headless_server();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();
    drop(foreground_control_rx);
    foreground_tx.test_close();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            foreground_tx,
        ),
    );
    server.foreground_client_id = Some(1);

    let changed = server.handle_internal_event_with_forwarding(TerminalEvent::ClipboardWrite {
        content: b"test".to_vec(),
    });

    assert!(!changed);
    assert!(
        !server.clients.contains_key(&1),
        "failed targeted send should remove the broken foreground client"
    );
}

#[test]
fn semantic_notifications_broadcast_to_all_client_shells() {
    let mut server = test_headless_server();
    let (shell_one_tx, shell_one_control, _shell_one_frames) = test_client_writer();
    let (shell_two_tx, shell_two_control, _shell_two_frames) = test_client_writer();
    for (client_id, writer) in [(1, shell_one_tx), (2, shell_two_tx)] {
        server.clients.insert(
            client_id,
            ClientConnection::new(
                (80, 24),
                crate::protocol::kitty::HostCellSize::default(),
                client_id,
                writer,
            ),
        );
    }
    let event = protocol::SemanticNotification {
        kind: protocol::SemanticNotificationKind::Custom,
        title: "hello".into(),
        body: None,
        sound: None,
        agent: None,
        workspace_id: None,
        tab_id: None,
        pane_id: None,
        position: None,
    };
    assert!(server.send_to_client_shells(ServerMessage::SemanticNotification(event.clone())));
    for receiver in [shell_one_control, shell_two_control] {
        assert_eq!(
            read_server_message(
                receiver
                    .recv_timeout(Duration::from_millis(100))
                    .expect("semantic notification")
            ),
            ServerMessage::SemanticNotification(event.clone())
        );
    }
}

#[test]
fn notification_show_uses_client_shell_policy_independent_of_server_delivery() {
    let mut server = test_headless_server();
    server.app.state.toast_config.delivery = config::ToastDelivery::Off;
    let (shell_tx, shell_control, _shell_frames) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            shell_tx,
        ),
    );
    let response = server.handle_notification_show_api(
        "notify-shell".into(),
        api::schema::NotificationShowParams {
            title: "plugin title".into(),
            body: Some("plugin body".into()),
            position: Some(crate::utils::config::ToastBusPosition::TopLeft),
            sound: api::schema::NotificationShowSound::Done,
        },
    );
    let response: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(
        response.result,
        api::schema::ResponseResult::NotificationShow { shown: true, .. }
    ));
    assert_eq!(
        read_server_message(
            shell_control
                .recv_timeout(Duration::from_millis(100))
                .expect("semantic plugin notification")
        ),
        ServerMessage::SemanticNotification(protocol::SemanticNotification {
            kind: protocol::SemanticNotificationKind::Custom,
            title: "plugin title".into(),
            body: Some("plugin body".into()),
            sound: Some(protocol::SemanticNotificationSound::Done),
            agent: None,
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            position: Some(crate::utils::config::ToastBusPosition::TopLeft),
        })
    );
}

#[test]
fn client_local_notifications_target_foreground_client_only() {
    let mut server = test_headless_server();
    let (background_tx, background_control_rx, _background_rx) = test_client_writer();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            background_tx,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            2,
            foreground_tx,
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    assert!(server.send_to_foreground_client(ServerMessage::Notify {
        kind: protocol::NotifyKind::Toast,
        message: "pi finished".to_string(),
        body: Some("workspace 1".to_string()),
    }));

    match read_server_message(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("foreground toast message"),
    ) {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::Toast);
            assert_eq!(message, "pi finished");
            assert_eq!(body.as_deref(), Some("workspace 1"));
        }
        other => panic!("expected toast notify, got {other:?}"),
    }
    assert!(
        background_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "background client should not receive client-local notifications"
    );
}

#[test]
fn notification_show_api_forwards_one_semantic_client_notification() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            client_tx,
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::utils::config::ToastDelivery::System;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed =
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: Some("api workspace".into()),
                        position: Some(crate::utils::config::ToastBusPosition::TopLeft),
                        sound: api::schema::NotificationShowSound::Request,
                    },
                ),
            },
            respond_to,
        });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("semantic api notification"),
    ) {
        ServerMessage::SemanticNotification(notification) => {
            assert_eq!(notification.title, "build failed");
            assert_eq!(notification.body.as_deref(), Some("api workspace"));
            assert_eq!(
                notification.sound,
                Some(protocol::SemanticNotificationSound::Request)
            );
        }
        other => panic!("expected semantic api notification, got {other:?}"),
    }
}

#[test]
fn notification_show_api_preserves_colon_in_forwarded_title() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            client_tx,
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::utils::config::ToastDelivery::System;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed =
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "build: failed".into(),
                        body: Some("api workspace".into()),
                        position: None,
                        sound: api::schema::NotificationShowSound::None,
                    },
                ),
            },
            respond_to,
        });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("semantic api notification"),
    ) {
        ServerMessage::SemanticNotification(notification) => {
            assert_eq!(notification.title, "build: failed");
            assert_eq!(notification.body.as_deref(), Some("api workspace"));
        }
        other => panic!("expected semantic api notification, got {other:?}"),
    }
}

#[test]
fn notification_show_api_validates_empty_title_before_disabled_delivery() {
    let mut server = test_headless_server();
    server.app.state.toast_config.delivery = crate::utils::config::ToastDelivery::Off;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed =
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "\n\t".into(),
                        body: None,
                        position: None,
                        sound: api::schema::NotificationShowSound::None,
                    },
                ),
            },
            respond_to,
        });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed.error.code, "invalid_params");
    assert_eq!(parsed.error.message, "notification title is empty");
}

#[test]
fn notification_show_api_reports_no_foreground_client() {
    let mut server = test_headless_server();
    server.foreground_client_id = None;
    server.app.state.toast_config.delivery = crate::utils::config::ToastDelivery::System;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed =
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: api::schema::NotificationShowSound::Request,
                    },
                ),
            },
            respond_to,
        });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: false,
            reason: api::schema::NotificationShowReason::NoForegroundClient,
        }
    );
}

#[test]
fn notification_show_api_includes_sound_in_semantic_event() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            client_tx,
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::utils::config::ToastDelivery::Bus;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    assert!(
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: api::schema::NotificationShowSound::Done,
                    },
                ),
            },
            respond_to,
        })
    );

    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("semantic api notification"),
    ) {
        ServerMessage::SemanticNotification(notification) => {
            assert_eq!(notification.title, "build failed");
            assert_eq!(
                notification.sound,
                Some(protocol::SemanticNotificationSound::Done)
            );
        }
        other => panic!("expected semantic api notification, got {other:?}"),
    }
}

#[test]
fn startup_idle_does_not_forward_completion() {
    let mut server = test_headless_server();
    let workspace = crate::server::workspaces::Workspace::test_new("active");
    let pane_id = workspace.tabs[0].root_pane;
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.toast_config.delivery = crate::utils::config::ToastDelivery::System;
    server.app.state.toast_config.delay_seconds = 0;
    server.app.state.sound.enabled = true;

    assert!(
        server.handle_internal_event_with_forwarding(TerminalEvent::AgentProcessDetected {
            pane_id,
            agent: crate::agents::AgentKind::Pi,
            observed_at: Instant::now(),
        })
    );

    let (client_tx, client_control_rx, _client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            client_tx,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    while client_control_rx
        .recv_timeout(Duration::from_millis(20))
        .is_ok()
    {}

    assert!(
        server.handle_internal_event_with_forwarding(TerminalEvent::StateChanged {
            pane_id,
            agent: Some(crate::agents::AgentKind::Pi),
            state: crate::agents::AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: Instant::now(),
        })
    );
    assert!(
        client_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "startup readiness should not forward a completion notification"
    );
}

/// Verify that calls to the app's internal-event methods only occur inside
/// `handle_internal_event_with_forwarding`. This ensures the forwarding
/// bypass cannot be reintroduced.
#[test]
fn no_handle_internal_event_bypass_in_module() {
    // Cover every former headless child so moving a call cannot hide a bypass.
    let sources = [
        ("main_loop.rs", include_str!("../main_loop.rs")),
        ("startup.rs", include_str!("../startup.rs")),
        ("shutdown.rs", include_str!("../shutdown.rs")),
        ("clients/focus.rs", include_str!("../clients/focus.rs")),
        (
            "clients/requests.rs",
            include_str!("../clients/requests.rs"),
        ),
        (
            "notifications/delivery.rs",
            include_str!("../notifications/delivery.rs"),
        ),
        (
            "clients/surface_lease.rs",
            include_str!("../clients/surface_lease.rs"),
        ),
        ("rendering/full.rs", include_str!("../rendering/full.rs")),
        (
            "rendering/incremental.rs",
            include_str!("../rendering/incremental.rs"),
        ),
        (
            "clients/foreground.rs",
            include_str!("../clients/foreground.rs"),
        ),
        ("clients/writer.rs", include_str!("../clients/writer.rs")),
        (
            "clients/events/mod.rs",
            include_str!("../clients/events/mod.rs"),
        ),
        (
            "clients/events/connection.rs",
            include_str!("../clients/events/connection.rs"),
        ),
        (
            "clients/events/shell.rs",
            include_str!("../clients/events/shell.rs"),
        ),
        (
            "api/server_methods.rs",
            include_str!("../api/server_methods.rs"),
        ),
        (
            "api/terminal_read.rs",
            include_str!("../api/terminal_read.rs"),
        ),
        (
            "rendering/window_title.rs",
            include_str!("../rendering/window_title.rs"),
        ),
    ];
    let mut bypass_lines: Vec<String> = Vec::new();

    for (file, source) in sources {
        let mut inside_forwarding_method = false;
        let mut forwarding_method_brace_depth = 0u32;

        for (i, line) in source.lines().enumerate() {
            let line_num = i + 1;

            // Track when we're inside handle_internal_event_with_forwarding.
            if line.contains("fn handle_internal_event_with_forwarding") {
                inside_forwarding_method = true;
                forwarding_method_brace_depth = 0;
            }

            if inside_forwarding_method {
                // Count braces to track when we exit the method.
                for ch in line.chars() {
                    match ch {
                        '{' => forwarding_method_brace_depth += 1,
                        '}' => {
                            forwarding_method_brace_depth =
                                forwarding_method_brace_depth.saturating_sub(1);
                            if forwarding_method_brace_depth == 0 {
                                inside_forwarding_method = false;
                            }
                        }
                        _ => {}
                    }
                }
            } else if (line.contains("self.app.handle_internal_event(")
                || line.contains("self.app.handle_internal_event_with_render_impact("))
                && !line.trim().starts_with("///")
                && !line.contains("contains(")
            {
                bypass_lines.push(format!("{file}:line {line_num}: {}", line.trim()));
            }
        }
    }

    assert!(
        bypass_lines.is_empty(),
        "Found direct calls to self.app.handle_internal_event outside \
             handle_internal_event_with_forwarding (bypass risk):\n  {}",
        bypass_lines.join("\n  ")
    );
}

#[test]
fn scheduled_tasks_settle_due_managed_agent_deadline_so_idle_loop_does_not_spin() {
    let mut server = test_headless_server();
    let workspace = crate::server::workspaces::Workspace::test_new("active");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);

    let started = Instant::now();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal")
        .begin_managed_agent(
            "worker".into(),
            crate::agents::AgentKind::Codex,
            started,
            Duration::from_secs(3),
            Duration::from_secs(30),
        );

    // The managed agent stays silent: no terminal event arrives after the settle delay.
    let before_settle = started + Duration::from_secs(2);
    assert!(!server.handle_scheduled_tasks_headless(before_settle, false));
    assert!(!server.app.state.session_dirty);
    assert!(server.app.event_hub.events_after(0).is_empty());

    let after_settle = started + Duration::from_secs(3) + Duration::from_millis(1);
    assert!(server.handle_scheduled_tasks_headless(after_settle, false));
    let next = server.app.next_headless_loop_deadline(after_settle, false);
    assert!(
        next.is_none_or(|deadline| deadline > after_settle),
        "a due managed-agent deadline must be consumed by the scheduler, \
         otherwise the loop sleeps until a past instant and spins: next={next:?} now={after_settle:?}"
    );
    assert!(server.app.state.session_dirty);
    assert_eq!(server.app.event_hub.events_after(0).len(), 1);

    let after_timeout = started + Duration::from_secs(30) + Duration::from_millis(1);
    assert!(server.handle_scheduled_tasks_headless(after_timeout, false));
    assert_eq!(
        server.app.state.terminals[&terminal_id].next_managed_agent_deadline(),
        None,
        "a silent managed launch past its timeout should be released by the scheduler"
    );
    assert!(server.app.state.terminals[&terminal_id]
        .agent_name
        .is_none());
    let events = server.app.event_hub.events_after(0);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|(_, event)| matches!(
        event.event,
        crate::protocol::api::schema::EventKind::PaneUpdated
    )));
    assert!(!server.handle_scheduled_tasks_headless(after_timeout, false));
    assert_eq!(server.app.event_hub.events_after(0).len(), 2);
}

#[test]
fn scheduled_tasks_make_a_silent_idle_managed_agent_ready_after_settle() {
    let mut server = test_headless_server();
    let workspace = crate::server::workspaces::Workspace::test_new("agent");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let unrelated = crate::server::workspaces::Workspace::test_new("unrelated");
    let unrelated_pane = unrelated.tabs[0].root_pane;
    let unrelated_terminal_id = unrelated.tabs[0].panes[&unrelated_pane]
        .attached_terminal_id
        .clone();
    server.app.state.workspaces = vec![workspace, unrelated];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let started = Instant::now();
    let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.begin_managed_agent(
        "worker".into(),
        crate::agents::AgentKind::Codex,
        started,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    terminal.set_detected_state(
        Some(crate::agents::AgentKind::Codex),
        crate::agents::AgentState::Idle,
    );
    let later_terminal = server
        .app
        .state
        .terminals
        .get_mut(&unrelated_terminal_id)
        .unwrap();
    later_terminal.begin_managed_agent(
        "later".into(),
        crate::agents::AgentKind::Codex,
        started,
        Duration::from_secs(20),
        Duration::from_secs(30),
    );
    later_terminal.set_detected_state(
        Some(crate::agents::AgentKind::Codex),
        crate::agents::AgentState::Idle,
    );

    let after_settle = started + Duration::from_secs(3);
    assert!(server.handle_scheduled_tasks_headless(after_settle, false));
    let terminal = &server.app.state.terminals[&terminal_id];
    assert!(terminal.managed_agent_interactive_ready());
    assert_eq!(terminal.agent_name.as_deref(), Some("worker"));
    assert_eq!(terminal.next_managed_agent_deadline(), None);
    let later_terminal = &server.app.state.terminals[&unrelated_terminal_id];
    assert!(later_terminal.managed_agent_launch_pending());
    assert_eq!(
        later_terminal.next_managed_agent_deadline(),
        Some(started + Duration::from_secs(20))
    );
    assert!(server.app.state.session_dirty);
    let events = server.app.event_hub.events_after(0);
    assert_eq!(events.len(), 1, "only the affected pane should be updated");
    let crate::protocol::api::schema::EventData::PaneUpdated { pane } = &events[0].1.data else {
        panic!("expected readiness to publish a pane update");
    };
    assert_eq!(pane.pane_id, server.app.public_pane_id(0, pane_id).unwrap());
    assert!(!server.handle_scheduled_tasks_headless(after_settle, false));
}
