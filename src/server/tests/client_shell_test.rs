use super::*;

#[tokio::test]
async fn client_shell_attach_seeds_workspace() {
    let mut server = test_headless_server();
    server.app.state.workspaces.clear();
    server.app.state.active = None;
    server.app.state.mode = crate::server::app_settings::Mode::Navigate;
    let (writer, _control_rx, _render_rx) = test_client_writer();

    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id: 6,
            surface_cols: 80,
            surface_rows: 23,
            cell_width_px: 0,
            cell_height_px: 0,
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: false,
            surface_active: true,
            writer,
        })
    );

    assert_eq!(
        server.app.state.mode,
        crate::server::app_settings::Mode::Terminal
    );
    assert_eq!(server.app.state.workspaces.len(), 1);
    assert_eq!(server.app.state.active, Some(0));
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_endpoint_request_uses_the_selected_connection() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::server::workspaces::Workspace::test_new("endpoint")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let (writer, control_rx, _render_rx) = test_client_writer();
    let client_id = 41;
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id,
            surface_cols: 80,
            surface_rows: 23,
            cell_width_px: 0,
            cell_height_px: 0,
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: false,
            surface_active: true,
            writer,
        })
    );
    let _initial_snapshot = control_rx.recv().expect("initial shell snapshot");
    let boot_id = server.client_shell_boot_id.clone();

    let workspace_id = server.app.state.workspaces[0].id.clone();
    server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
        client_id,
        boot_id: boot_id.clone(),
        request: Box::new(api::schema::Request {
            id: "client-shell:1".into(),
            method: api::schema::Method::WorkspaceRename(api::schema::WorkspaceRenameParams {
                workspace_id: workspace_id.clone(),
                label: "renamed".into(),
            }),
        }),
    });
    assert!(server.clients[&client_id].shell_endpoint_command_in_flight);

    assert!(
        !server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id: boot_id.clone(),
            request: Box::new(api::schema::Request {
                id: "client-shell:busy".into(),
                method: api::schema::Method::WorkspaceRename(api::schema::WorkspaceRenameParams {
                    workspace_id,
                    label: "busy".into(),
                }),
            }),
        })
    );
    assert!(server.clients.contains_key(&client_id));
    let ServerMessage::ClientShellEndpointResponseChunk { data, .. } =
        read_server_message(control_rx.recv().expect("busy endpoint response"))
    else {
        panic!("expected busy endpoint response");
    };
    let response =
        serde_json::from_slice::<api::schema::ErrorResponse>(&data).expect("typed busy response");
    assert_eq!(response.error.code, "endpoint_busy");

    let response_ready = server
        .server_event_rx
        .recv()
        .await
        .expect("endpoint response ready");
    assert!(!server.handle_server_event(response_ready));
    assert!(!server.clients[&client_id].shell_endpoint_command_in_flight);

    match read_server_message(control_rx.recv().expect("endpoint response")) {
        ServerMessage::ClientShellEndpointResponseChunk {
            boot_id: response_boot_id,
            request_id,
            final_chunk,
            data,
        } => {
            assert_eq!(response_boot_id, boot_id);
            assert_eq!(request_id, "client-shell:1");
            assert!(final_chunk);
            let response = serde_json::from_slice::<api::schema::SuccessResponse>(&data)
                .expect("success response");
            assert_eq!(response.id, "client-shell:1");
        }
        other => panic!("expected client shell endpoint response, got {other:?}"),
    }
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_receives_metadata_then_shell_free_pane_surface() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("shell-only-label");
    let pane_id = workspace.focused_pane_id().expect("focused pane");
    workspace.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(
            80,
            23,
            b"\x1b[?1003h\x1b[?1006h\x1b[?1016hCLIENT_SHELL_LIVE",
        ),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    server.server_config_diagnostic = Some("endpoint config warning".into());

    let (writer, control_rx, render_rx) = test_client_writer();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id: 7,
            surface_cols: 80,
            surface_rows: 23,
            cell_width_px: 10,
            cell_height_px: 20,
            pixel_mouse: true,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: false,
            surface_active: true,
            writer,
        })
    );
    let snapshot = client_shell_snapshot(read_server_message(
        control_rx.recv().expect("shell snapshot"),
    ));
    assert_eq!(snapshot.workspaces.len(), 1);
    assert_eq!(snapshot.workspaces[0].label, "shell-only-label");
    assert_eq!(
        snapshot.config_diagnostic.as_deref(),
        Some("endpoint config warning")
    );

    server.render_and_stream();
    let initial_surface = match read_server_message(render_rx.recv().expect("pane surface")) {
        ServerMessage::PaneSurface(surface) => {
            assert_eq!((surface.frame.width, surface.frame.height), (80, 23));
            let text = frame_text(&surface.frame);
            assert!(text.contains("CLIENT_SHELL_LIVE"), "surface: {text:?}");
            assert!(!text.contains("shell-only-label"), "surface: {text:?}");
            assert_eq!(surface.panes.len(), 1);
            assert_eq!(surface.panes[0].rect.x, 0);
            assert_eq!(surface.panes[0].rect.y, 0);
            assert!(surface.panes[0].sgr_pixel_mouse);
            assert_eq!(
                surface.panes[0].pixel_width,
                u32::from(surface.panes[0].inner_rect.width) * 10
            );
            assert_eq!(
                surface.panes[0].pixel_height,
                u32::from(surface.panes[0].inner_rect.height) * 20
            );
            surface
        }
        other => panic!("expected pane surface, got {other:?}"),
    };

    server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("pane runtime")
        .test_process_pty_bytes(b"\rPATCHED");
    let sources = std::collections::HashSet::from([pane_id]);
    assert!(server.render_retained_pane_surface_and_stream(&sources));
    match read_server_message(render_rx.recv().expect("pane surface patch")) {
        ServerMessage::PaneSurfacePatch(patch) => {
            assert_eq!(
                patch.base_surface_revision,
                initial_surface.surface_revision
            );
            assert_eq!(patch.surface_revision, initial_surface.surface_revision + 1);
            assert_eq!(patch.panes.len(), 1);
            assert!(!patch.rows.is_empty());
            assert!(patch
                .rows
                .iter()
                .flat_map(|row| &row.cells)
                .any(|cell| cell.symbol == "P"));
        }
        other => panic!("expected pane surface patch, got {other:?}"),
    }
    server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("pane runtime")
        .test_process_pty_bytes(b"\x1b[?1003l\x1b[?1006l\x1b[?1016l");
    assert!(server.render_retained_pane_surface_and_stream(&sources));
    match read_server_message(render_rx.recv().expect("metadata-only pane surface patch")) {
        ServerMessage::PaneSurfacePatch(patch) => {
            assert_eq!(patch.panes.len(), 1);
            assert!(!patch.panes[0].mouse_reporting);
            assert!(!patch.panes[0].sgr_pixel_mouse);
        }
        other => panic!("expected metadata-only pane surface patch, got {other:?}"),
    }
    let retained = server.clients[&7]
        .render_state
        .last_pane_surface()
        .expect("committed retained surface")
        .clone();
    server
        .clients
        .get_mut(&7)
        .unwrap()
        .render_state
        .request_repaint();
    server.render_and_stream();
    let full = match read_server_message(render_rx.recv().expect("full comparison surface")) {
        ServerMessage::PaneSurface(surface) => surface,
        other => panic!("expected full comparison surface, got {other:?}"),
    };
    assert!(full.surface_revision > retained.surface_revision);
    assert_eq!(retained.frame, full.frame);
    assert_eq!(retained.panes, full.panes);
    assert_eq!(retained.splits, full.splits);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_tabs_render_accept_input_and_resize_independently() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("independent-geometry");
    let first_pane = workspace.tabs[0].root_pane;
    let second_tab = workspace.test_add_tab(Some("second"));
    let second_pane = workspace.tabs[second_tab].root_pane;
    workspace.insert_test_runtime(
        first_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"FIRST_TAB"),
    );
    let (second_runtime, mut second_input) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"SECOND_TAB",
            4,
        );
    workspace.insert_test_runtime(second_pane, second_runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let second_tab_id = server.app.public_tab_id(0, second_tab).unwrap();
    let second_pane_id = server.app.public_pane_id(0, second_pane).unwrap();
    let initial_second_size =
        server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();

    let (first_control, first_render) = connect_test_shell(&mut server, 21, 100, 30);
    let _ = first_control.recv().expect("first snapshot");
    let first_size = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    let singleton_second_size =
        server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();
    assert_ne!(singleton_second_size, initial_second_size);
    assert_eq!(singleton_second_size, first_size);

    let (second_control, second_render) = connect_test_shell(&mut server, 22, 70, 20);
    let _ = second_control.recv().expect("second snapshot");

    assert!(server.focus_shell_client_on_tab(22, &second_tab_id));
    assert!(server.claim_shell_tab_geometry(22, false));
    let second_size = server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();
    assert_ne!(first_size, second_size);
    assert_eq!(
        server.app.state.workspaces[0].test_runtimes[&first_pane].current_size(),
        first_size
    );

    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 22,
        pane_id: second_pane_id,
        events: vec![crate::protocol::wire::ClientPaneInputEvent::TextCommit(
            "typed".into(),
        )],
    });
    assert_eq!(
        second_input.try_recv().expect("second tab input"),
        Bytes::from_static(b"typed")
    );

    server.render_and_stream();
    let first_surface = match read_server_message(first_render.recv().expect("first surface")) {
        ServerMessage::PaneSurface(surface) => surface,
        other => panic!("expected first pane surface, got {other:?}"),
    };
    let second_surface = match read_server_message(second_render.recv().expect("second surface")) {
        ServerMessage::PaneSurface(surface) => surface,
        other => panic!("expected second pane surface, got {other:?}"),
    };
    assert!(frame_text(&first_surface.frame).contains("FIRST_TAB"));
    assert!(frame_text(&second_surface.frame).contains("SECOND_TAB"));

    assert!(server.handle_server_event(ServerEvent::ClientShellResize {
        client_id: 22,
        surface_cols: 60,
        surface_rows: 16,
        cell_width_px: 0,
        cell_height_px: 0,
        pixel_mouse: false,
    }));
    let resized_second = server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();
    assert_ne!(resized_second, second_size);
    assert_eq!(
        server.app.state.workspaces[0].test_runtimes[&first_pane].current_size(),
        first_size
    );

    assert!(server.focus_shell_client_on_tab(21, &second_tab_id));
    assert!(server.claim_shell_tab_geometry(21, false));
    assert_ne!(
        server.app.state.workspaces[0].test_runtimes[&second_pane].current_size(),
        resized_second
    );

    server.remove_client_and_resize_if_needed(21);
    let singleton_first = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    let singleton_second =
        server.app.state.workspaces[0].test_runtimes[&second_pane].current_size();
    assert_ne!(singleton_first, first_size);
    assert_eq!(singleton_first, singleton_second);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn public_background_tab_create_preserves_client_locations() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("background-create");
    let second_tab = workspace.test_add_tab(Some("second"));
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let workspace_id = server.app.public_workspace_id(0);
    let first_tab_id = server.app.public_tab_id(0, 0).unwrap();
    let second_tab_id = server.app.public_tab_id(0, second_tab).unwrap();

    let (first_control, _) = connect_matching_test_shell(&mut server, 71);
    let (second_control, _) = connect_matching_test_shell(&mut server, 72);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");
    assert!(server.focus_shell_client_on_tab(71, &second_tab_id));

    let (respond_to, _response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
        request: crate::protocol::api::schema::Request {
            id: "create-background-tab".into(),
            method: crate::protocol::api::schema::Method::TabCreate(
                crate::protocol::api::schema::TabCreateParams {
                    workspace_id: Some(workspace_id),
                    cwd: None,
                    focus: false,
                    label: Some("background".into()),
                    env: std::collections::HashMap::new(),
                },
            ),
        },
        respond_to,
    });

    assert_eq!(
        server.shell_tab_id_for_client(71).as_deref(),
        Some(second_tab_id.as_str())
    );
    assert_eq!(
        server.shell_tab_id_for_client(72).as_deref(),
        Some(first_tab_id.as_str())
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn public_workspace_focus_preserves_each_clients_remembered_tabs() {
    let mut server = test_headless_server();
    let mut first = crate::server::workspaces::Workspace::test_new("first");
    let second_tab = first.test_add_tab(Some("second"));
    let second = crate::server::workspaces::Workspace::test_new("second");
    server.app.state.workspaces = vec![first, second];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let first_workspace_id = server.app.public_workspace_id(0);
    let second_workspace_id = server.app.public_workspace_id(1);
    let first_tab_id = server.app.public_tab_id(0, 0).unwrap();
    let second_tab_id = server.app.public_tab_id(0, second_tab).unwrap();

    let (first_control, _) = connect_test_shell(&mut server, 41, 100, 30);
    let (second_control, _) = connect_test_shell(&mut server, 42, 80, 24);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");
    assert!(server.focus_shell_client_on_tab(41, &second_tab_id));

    let (respond_to, _response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
        request: crate::protocol::api::schema::Request {
            id: "focus-second-workspace".into(),
            method: crate::protocol::api::schema::Method::WorkspaceFocus(
                crate::protocol::api::schema::WorkspaceTarget {
                    workspace_id: second_workspace_id.clone(),
                },
            ),
        },
        respond_to,
    });

    let first_location = server.clients[&41].shell_location.as_ref().unwrap();
    let second_location = server.clients[&42].shell_location.as_ref().unwrap();
    assert_eq!(
        first_location.focused_workspace_id.as_deref(),
        Some(second_workspace_id.as_str())
    );
    assert_eq!(
        second_location.focused_workspace_id.as_deref(),
        Some(second_workspace_id.as_str())
    );
    assert_eq!(
        first_location.active_tab_ids[&first_workspace_id],
        second_tab_id
    );
    assert_eq!(
        second_location.active_tab_ids[&first_workspace_id],
        first_tab_id
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn public_api_focus_replaces_every_client_shell_projection() {
    let mut server = test_headless_server();
    let first = crate::server::workspaces::Workspace::test_new("first");
    let second = crate::server::workspaces::Workspace::test_new("second");
    server.app.state.workspaces = vec![first, second];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let second_id = server.app.session_snapshot().workspaces[1]
        .workspace_id
        .clone();

    let (writer, control_rx, render_rx) = test_client_writer();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id: 9,
            surface_cols: 80,
            surface_rows: 23,
            cell_width_px: 0,
            cell_height_px: 0,
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: false,
            surface_active: true,
            writer,
        })
    );
    let initial_revision = client_shell_snapshot(read_server_message(
        control_rx.recv().expect("initial snapshot"),
    ))
    .revision;

    let (respond_to, _response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
        request: crate::protocol::api::schema::Request {
            id: "test.client.shell.workspace.focus".into(),
            method: crate::protocol::api::schema::Method::WorkspaceFocus(
                crate::protocol::api::schema::WorkspaceTarget {
                    workspace_id: second_id.clone(),
                },
            ),
        },
        respond_to,
    });
    assert_eq!(server.app.state.active, Some(1));
    server.render_and_stream();

    let replacement = client_shell_snapshot(read_server_message(
        control_rx.recv().expect("replacement snapshot"),
    ));
    assert!(replacement.revision > initial_revision);
    assert_eq!(
        replacement.focused_workspace_id.as_deref(),
        Some(second_id.as_str())
    );
    match read_server_message(render_rx.recv().expect("replacement pane surface")) {
        ServerMessage::PaneSurface(surface) => {
            assert_eq!(surface.projection_revision, replacement.revision);
        }
        other => panic!("expected replacement pane surface, got {other:?}"),
    }
    shutdown_test_runtimes(&mut server);
}

#[test]
fn client_shell_host_theme_follows_foreground_client() {
    let mut server = test_headless_server();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            2,
            unread_test_writer(),
        ),
    );
    server.foreground_client_id = Some(1);

    let dark = protocol::ClientHostColor {
        r: 20,
        g: 30,
        b: 40,
    };
    let blue = protocol::ClientHostColor {
        r: 10,
        g: 20,
        b: 200,
    };
    assert!(
        server.handle_server_event(ServerEvent::ClientShellHostTheme {
            client_id: 1,
            update: protocol::ClientHostThemeUpdate::DefaultColor {
                kind: protocol::ClientHostDefaultColorKind::Background,
                color: dark,
            },
        })
    );
    assert!(
        server.handle_server_event(ServerEvent::ClientShellHostTheme {
            client_id: 1,
            update: protocol::ClientHostThemeUpdate::PaletteColors(vec![(4, blue)]),
        })
    );
    server.handle_server_event(ServerEvent::ClientShellHostTheme {
        client_id: 1,
        update: protocol::ClientHostThemeUpdate::Appearance(protocol::ClientHostAppearance::Dark),
    });
    assert_eq!(
        server.app.state.host_terminal_theme.background,
        Some(dark.into())
    );
    assert_eq!(
        server.app.state.host_terminal_theme.palette[4],
        Some(blue.into())
    );
    assert_eq!(
        server.app.state.host_terminal_appearance,
        Some(crate::utils::theme::color::HostAppearance::Dark)
    );
    assert!(server.app.state.host_terminal_appearance_explicit);

    let light = protocol::ClientHostColor {
        r: 240,
        g: 230,
        b: 220,
    };
    assert!(
        !server.handle_server_event(ServerEvent::ClientShellHostTheme {
            client_id: 2,
            update: protocol::ClientHostThemeUpdate::DefaultColor {
                kind: protocol::ClientHostDefaultColorKind::Background,
                color: light,
            },
        })
    );
    assert_eq!(
        server.app.state.host_terminal_theme.background,
        Some(dark.into())
    );

    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();
    assert_eq!(
        server.app.state.host_terminal_theme.background,
        Some(light.into())
    );
    assert_eq!(
        server.app.state.host_terminal_appearance,
        Some(crate::utils::theme::color::HostAppearance::Light)
    );
    assert!(!server.app.state.host_terminal_appearance_explicit);
}

#[test]
fn client_shell_stores_known_cell_geometry_independently_of_pixel_mouse() {
    with_terminal_session_test_server(|server, _, _, _| {
        let (writer, _control_rx, _render_rx) = test_client_writer();
        assert!(
            server.handle_server_event(ServerEvent::ClientShellConnected {
                client_id: 7,
                surface_cols: 80,
                surface_rows: 24,
                cell_width_px: 0,
                cell_height_px: 0,
                pixel_mouse: true,
                direct_graphics: false,
                endpoint_keybindings: false,
                mouse_capture: false,
                surface_active: true,
                writer,
            })
        );
        assert!(!server.clients[&7].pixel_mouse);
        assert_eq!(
            server.clients[&7].cell_size,
            crate::protocol::kitty::HostCellSize::default()
        );

        let (writer, _control_rx, _render_rx) = test_client_writer();
        assert!(
            server.handle_server_event(ServerEvent::ClientShellConnected {
                client_id: 8,
                surface_cols: 80,
                surface_rows: 24,
                cell_width_px: 10,
                cell_height_px: 20,
                pixel_mouse: false,
                direct_graphics: false,
                endpoint_keybindings: false,
                mouse_capture: false,
                surface_active: true,
                writer,
            })
        );
        assert!(!server.clients[&8].pixel_mouse);
        assert_eq!(
            server.clients[&8].cell_size,
            crate::protocol::kitty::HostCellSize {
                width_px: 10,
                height_px: 20,
            }
        );
    });
}

#[tokio::test]
async fn client_shell_tab_focus_changes_only_the_source_connection() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("independent-tabs");
    let second_tab = workspace.test_add_tab(Some("second"));
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let tab_ids = server
        .app
        .session_snapshot()
        .tabs
        .into_iter()
        .map(|tab| tab.tab_id)
        .collect::<Vec<_>>();
    let first_tab_id = tab_ids[0].clone();
    let second_tab_id = tab_ids[second_tab].clone();

    let (first_control, _first_render) = connect_test_shell(&mut server, 7, 100, 30);
    let (second_control, _second_render) = connect_test_shell(&mut server, 8, 80, 24);
    let first_initial = client_shell_snapshot(read_server_message(
        first_control.recv().expect("first snapshot"),
    ));
    let second_initial = client_shell_snapshot(read_server_message(
        second_control.recv().expect("second snapshot"),
    ));
    assert_eq!(
        first_initial.focused_tab_id.as_deref(),
        Some(first_tab_id.as_str())
    );
    assert_eq!(
        second_initial.focused_tab_id.as_deref(),
        Some(first_tab_id.as_str())
    );

    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id: 8,
            boot_id: server.client_shell_boot_id.clone(),
            request: Box::new(api::schema::Request {
                id: "focus-second".into(),
                method: api::schema::Method::TabFocus(api::schema::TabTarget {
                    tab_id: second_tab_id.clone(),
                }),
            }),
        })
    );
    let response_ready = server
        .server_event_rx
        .recv()
        .await
        .expect("focus response ready");
    assert!(!server.handle_server_event(response_ready));
    let _ = second_control.recv().expect("focus response");

    server.render_and_stream();

    assert!(
        first_control.try_recv().is_err(),
        "another shell must not receive a navigation replacement"
    );
    let second_replacement = client_shell_snapshot(read_server_message(
        second_control.recv().expect("second replacement snapshot"),
    ));
    assert_eq!(
        second_replacement.focused_tab_id.as_deref(),
        Some(second_tab_id.as_str())
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_local_navigation_does_not_emit_global_focus_transitions() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("independent-focus");
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
    let second_tab_id = server.app.public_tab_id(0, second_tab).unwrap();

    let (first_control, _) = connect_matching_test_shell(&mut server, 61);
    let (second_control, _) = connect_matching_test_shell(&mut server, 62);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");
    assert!(server.focus_shell_client_on_tab(62, &second_tab_id));
    server.clients.get_mut(&61).unwrap().outer_terminal_focus = Some(true);
    server.clients.get_mut(&62).unwrap().outer_terminal_focus = Some(true);

    let (respond_to, _response_rx) = std::sync::mpsc::channel();
    server.handle_client_shell_api_request(
        62,
        crate::server::api::ApiRequestMessage {
            request: crate::protocol::api::schema::Request {
                id: "focus-own-tab".into(),
                method: crate::protocol::api::schema::Method::TabFocus(
                    crate::protocol::api::schema::TabTarget {
                        tab_id: second_tab_id,
                    },
                ),
            },
            respond_to,
        },
    );
    server.app.sync_focus_events();
    assert!(first_input.try_recv().is_err());
    assert!(second_input.try_recv().is_err());
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn public_close_reapplies_controller_geometry() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("public-close-geometry");
    let first_pane = workspace.tabs[0].root_pane;
    let second_pane = workspace.test_split(ratatui::layout::Direction::Vertical);
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
    let second_pane_id = server.app.public_pane_id(0, second_pane).unwrap();

    let (control, _) = connect_test_shell(&mut server, 66, 100, 30);
    let _ = control.recv().expect("snapshot");
    let shrunk = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    assert!(shrunk.0 < 30);

    let (respond_to, _response_rx) = std::sync::mpsc::channel();
    assert!(
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            request: crate::protocol::api::schema::Request {
                id: "public-close-geometry".into(),
                method: crate::protocol::api::schema::Method::PaneClose(
                    crate::protocol::api::schema::PaneTarget {
                        pane_id: second_pane_id,
                    }
                ),
            },
            respond_to,
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
