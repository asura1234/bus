#[tokio::test]
async fn public_focus_moves_shell_focus_between_tabs() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("public-focus-events");
    let first_pane = workspace.tabs[0].root_pane;
    let second_tab = workspace.test_add_tab(Some("second"));
    let second_pane = workspace.tabs[second_tab].root_pane;
    let (first_runtime, mut first_input) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[?1004h",
            4,
        );
    let (second_runtime, mut second_input) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[?1004h",
            4,
        );
    workspace.insert_test_runtime(first_pane, first_runtime);
    workspace.insert_test_runtime(second_pane, second_runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let second_tab_id = server
        .app
        .public_tab_id(0, second_tab)
        .expect("second tab id");

    let (first_control, _) = connect_matching_test_shell(&mut server, 63);
    let (second_control, _) = connect_matching_test_shell(&mut server, 64);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");
    assert!(server.focus_shell_client_on_tab(64, &second_tab_id));
    server.clients.get_mut(&63).unwrap().outer_terminal_focus = Some(true);
    server.clients.get_mut(&64).unwrap().outer_terminal_focus = Some(true);
    assert!(server.app.state.switch_workspace_tab(0, second_tab));

    server.focus_all_shell_clients_on_default_target();

    assert_eq!(
        server.shell_tab_id_for_client(63).as_deref(),
        Some(second_tab_id.as_str())
    );
    assert_eq!(
        first_input.try_recv().expect("previous tab focus lost"),
        Bytes::from_static(b"\x1b[O")
    );
    assert!(
        second_input.try_recv().is_err(),
        "focus gain was duplicated"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn repeated_layout_action_reapplies_controller_geometry() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("layout-geometry");
    let first_pane = workspace.tabs[0].root_pane;
    let second_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.insert_test_runtime(
        first_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    workspace.insert_test_runtime(
        second_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let tab_id = server.app.public_tab_id(0, 0).expect("tab id");

    let (control, _) = connect_test_shell(&mut server, 65, 100, 30);
    let _ = control.recv().expect("snapshot");
    let before = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    let (respond_to, _response_rx) = std::sync::mpsc::channel();

    assert!(server.handle_client_shell_api_request(
        65,
        crate::server::api::ApiRequestMessage {
            request: crate::protocol::api::schema::Request {
                id: "resize-layout".into(),
                method: crate::protocol::api::schema::Method::LayoutSetSplitRatio(
                    crate::protocol::api::schema::LayoutSetSplitRatioParams {
                        tab_id: Some(tab_id),
                        pane_id: None,
                        path: Vec::new(),
                        ratio: 0.8,
                    },
                ),
            },
            respond_to,
        },
    ));

    let after = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    assert_ne!(after, before);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn geometry_reapply_replaces_a_controller_that_left_the_tab() {
    let mut server = test_headless_server();
    let mut workspace =
        crate::server::workspaces::Workspace::test_new("geometry-controller-viewer");
    let first_pane = workspace.tabs[0].root_pane;
    let second_tab = workspace.test_add_tab(Some("second"));
    let second_pane = workspace.tabs[second_tab].root_pane;
    let third_tab = workspace.test_add_tab(Some("third"));
    let third_pane = workspace.tabs[third_tab].root_pane;
    for pane_id in [first_pane, second_pane, third_pane] {
        workspace.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
        );
    }
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let second_tab_id = server.app.public_tab_id(0, second_tab).unwrap();
    let third_tab_id = server.app.public_tab_id(0, third_tab).unwrap();

    let (first_control, _) = connect_test_shell(&mut server, 67, 100, 30);
    let (second_control, _) = connect_test_shell(&mut server, 68, 70, 20);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");

    assert!(server.focus_shell_client_on_tab(67, &second_tab_id));
    assert!(server.claim_shell_tab_geometry(67, false));
    assert!(server.focus_shell_client_on_tab(67, &third_tab_id));
    assert!(server.claim_shell_tab_geometry(67, false));
    assert!(server.focus_shell_client_on_tab(68, &second_tab_id));
    assert_eq!(
        server.tab_geometry_controllers.get(&second_tab_id),
        Some(&67)
    );
    let stale_size = server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();

    assert!(server.reapply_controlled_shell_tab_geometry(false));

    assert_eq!(
        server.tab_geometry_controllers.get(&second_tab_id),
        Some(&68)
    );
    assert_ne!(
        server.app.state.workspaces[0].test_runtimes[&second_pane].current_size(),
        stale_size
    );
    shutdown_test_runtimes(&mut server);
}

#[test]
fn room_orchestrator_core_headless_explicit_agent_history_read_requires_idle_on_alternate_screen() {
    with_terminal_session_test_server(
        |server, terminal_id, _terminal_id_string, public_pane_id| {
            let terminal = server
                .app
                .state
                .terminals
                .get_mut(&terminal_id)
                .expect("terminal");
            terminal.detected_agent = Some(crate::agents::AgentKind::Claude);
            terminal.state = crate::agents::AgentState::Working;
            server.app.terminal_runtimes.insert(
                terminal_id,
                crate::terminal::TerminalRuntime::test_with_screen_bytes(
                    80,
                    24,
                    b"\x1b[?1049hworking",
                ),
            );
            let request = api::schema::Request {
                id: "read".into(),
                method: api::schema::Method::AgentRead(api::schema::AgentReadParams {
                    target: public_pane_id.clone(),
                    source: api::schema::ReadSource::Recent,
                    lines: Some(200),
                    format: api::schema::ReadFormat::Text,
                    strip_ansi: true,
                }),
            };

            assert_eq!(
                    server.agent_read_not_idle_error(&request),
                    Some(api::schema::ErrorBody {
                        code: "agent_not_idle".into(),
                        message: format!(
                            "cannot read 200 lines while {public_pane_id} is working: its alternate-screen history can only be captured by scrolling while idle. Wait and retry, or use --source visible"
                        ),
                    })
                );

            let mut default_request = request.clone();
            let api::schema::Method::AgentRead(params) = &mut default_request.method else {
                unreachable!();
            };
            params.lines = None;
            assert_eq!(server.agent_read_not_idle_error(&default_request), None);

            let mut visible_request = request;
            let api::schema::Method::AgentRead(params) = &mut visible_request.method else {
                unreachable!();
            };
            params.source = api::schema::ReadSource::Visible;
            assert_eq!(server.agent_read_not_idle_error(&visible_request), None);
        },
    );
}

#[tokio::test]
async fn pane_death_reconciles_each_client_view_and_focus() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("pane-death-views");
    let dead_pane = workspace.tabs[0].root_pane;
    let second_tab = workspace.test_add_tab(Some("second"));
    let second_pane = workspace.tabs[second_tab].root_pane;
    let (second_runtime, mut second_input) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[?1004h",
            4,
        );
    workspace.insert_test_runtime(second_pane, second_runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let second_tab_id = server
        .app
        .public_tab_id(0, second_tab)
        .expect("second tab id");

    let (first_control, _) = connect_test_shell(&mut server, 71, 100, 30);
    let (second_control, _) = connect_test_shell(&mut server, 72, 70, 20);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");
    assert!(server.focus_shell_client_on_tab(72, &second_tab_id));
    server.clients.get_mut(&71).unwrap().outer_terminal_focus = Some(true);
    server.clients.get_mut(&72).unwrap().outer_terminal_focus = Some(false);

    assert!(
        server.handle_internal_event_with_forwarding(TerminalEvent::PaneDied {
            pane_id: dead_pane,
            exit_reason: crate::platform::ChildExitReason::Exited
        })
    );

    assert_eq!(
        server.shell_tab_id_for_client(71).as_deref(),
        Some(second_tab_id.as_str())
    );
    assert_eq!(
        server.shell_tab_id_for_client(72).as_deref(),
        Some(second_tab_id.as_str())
    );
    assert_eq!(
        second_input
            .try_recv()
            .expect("fallback focus gained input"),
        Bytes::from_static(b"\x1b[I")
    );
    assert!(
        second_input.try_recv().is_err(),
        "focus gain was duplicated"
    );
    assert_eq!(
        server.tab_geometry_controllers.get(&second_tab_id),
        Some(&71)
    );
    let before_resize = server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();
    assert!(server.handle_server_event(ServerEvent::ClientShellResize {
        client_id: 71,
        surface_cols: 90,
        surface_rows: 25,
        cell_width_px: 0,
        cell_height_px: 0,
        pixel_mouse: false,
    }));
    assert_ne!(
        server.app.state.workspaces[0].test_runtimes[&second_pane].current_size(),
        before_resize
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn pane_death_reapplies_controller_geometry() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("pane-death-geometry");
    let first_pane = workspace.tabs[0].root_pane;
    let dead_pane = workspace.test_split(ratatui::layout::Direction::Vertical);
    workspace.insert_test_runtime(
        first_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    workspace.insert_test_runtime(
        dead_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;

    let (control, _) = connect_test_shell(&mut server, 73, 185, 46);
    let _ = control.recv().expect("snapshot");
    let shrunk = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    assert!(shrunk.0 < 46);

    assert!(
        server.handle_internal_event_with_forwarding(TerminalEvent::PaneDied {
            pane_id: dead_pane,
            exit_reason: crate::platform::ChildExitReason::Exited
        })
    );

    let runtime = &server.app.state.workspaces[0].test_runtimes[&first_pane];
    let grown = runtime.current_size();
    assert!(grown.0 > shrunk.0);
    assert_eq!(runtime.terminal_dimensions(), Some((grown.1, grown.0)));
    assert_eq!(
        runtime.scroll_metrics().unwrap().viewport_rows,
        grown.0 as usize
    );
    shutdown_test_runtimes(&mut server);
}

#[test]
fn client_config_reload_request_refreshes_attached_clients() {
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
    server.app.state.request_client_config_reload = true;

    server.drain_client_config_reload_request();

    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("client config reload message"),
    ) {
        ServerMessage::ReloadSoundConfig => {}
        other => panic!("expected ReloadSoundConfig, got {other:?}"),
    }
    assert!(!server.app.state.request_client_config_reload);
}
