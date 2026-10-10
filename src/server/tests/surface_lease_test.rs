use super::*;

#[tokio::test]
async fn metadata_only_shell_is_isolated_until_surface_activation() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"");
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    let workspace_id = server.app.session_snapshot().focused_workspace_id.unwrap();
    let original_size = server.effective_size;
    let (writer, control_rx, render_rx) = test_client_writer();
    let client_id = 52;

    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id,
            surface_cols: 101,
            surface_rows: 37,
            cell_width_px: 9,
            cell_height_px: 18,
            pixel_mouse: true,
            direct_graphics: false,
            endpoint_keybindings: true,
            mouse_capture: true,
            surface_active: false,
            writer,
        })
    );
    assert!(matches!(
        read_server_message(control_rx.recv().expect("metadata snapshot")),
        ServerMessage::EndpointControl { .. }
    ));
    assert_eq!(server.foreground_client_id, None);
    assert_eq!(server.effective_size, original_size);

    server.render_and_stream();
    assert!(render_rx.try_recv().is_err());
    assert!(server.clients[&client_id]
        .render_state
        .last_pane_surface()
        .is_none());

    assert!(
        !server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id,
            pane_id,
            events: vec![crate::protocol::wire::ClientPaneInputEvent::Paste(
                "blocked".into()
            )],
        })
    );
    assert!(input_rx.try_recv().is_err());

    let boot_id = server.client_shell_boot_id.clone();
    assert!(
        !server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id: boot_id.clone(),
            request: Box::new(api::schema::Request {
                id: "inactive-mutation".into(),
                method: api::schema::Method::WorkspaceFocus(api::schema::WorkspaceTarget {
                    workspace_id,
                }),
            }),
        })
    );
    let ServerMessage::ClientShellEndpointResponseChunk { data, .. } =
        read_server_message(control_rx.recv().expect("inactive mutation response"))
    else {
        panic!("expected endpoint response");
    };
    let error = serde_json::from_slice::<api::schema::ErrorResponse>(&data).unwrap();
    assert_eq!(error.error.code, "surface_inactive");

    assert!(
        server.send_to_client_shells(ServerMessage::SemanticNotification(
            crate::protocol::wire::SemanticNotification {
                kind: crate::protocol::wire::SemanticNotificationKind::Custom,
                title: "metadata event".into(),
                body: None,
                sound: None,
                agent: None,
                workspace_id: None,
                tab_id: None,
                pane_id: None,
                position: None,
            },
        ))
    );
    assert!(matches!(
        read_server_message(control_rx.recv().expect("metadata notification")),
        ServerMessage::SemanticNotification(_)
    ));

    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id: boot_id.clone(),
            request: Box::new(api::schema::Request {
                id: "activate-surface".into(),
                method: api::schema::Method::ClientShellSurfaceSet(
                    api::schema::ClientShellSurfaceSetParams { active: true },
                ),
            }),
        })
    );
    let ServerMessage::ClientShellEndpointResponseChunk { data, .. } =
        read_server_message(control_rx.recv().expect("surface activation response"))
    else {
        panic!("expected typed surface activation response");
    };
    let activation_ack = serde_json::from_slice::<api::schema::SuccessResponse>(&data).unwrap();
    let api::schema::ResponseResult::ClientShellSurfaceSet {
        active: true,
        projection_revision: activation_floor,
    } = activation_ack.result
    else {
        panic!("expected typed surface activation result");
    };
    assert_eq!(server.foreground_client_id, Some(client_id));
    assert_eq!(server.effective_size, (101, 37));

    server.render_and_stream();
    let ServerMessage::PaneSurface(surface) =
        read_server_message(render_rx.recv().expect("activated surface"))
    else {
        panic!("expected pane surface");
    };
    assert_eq!((surface.frame.width, surface.frame.height), (101, 37));
    assert!(surface.projection_revision >= activation_floor);
    assert_eq!(surface.surface_revision, 1);
    server
        .clients
        .get_mut(&client_id)
        .expect("surface client")
        .shell_endpoint_command_in_flight = true;

    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id: boot_id.clone(),
            request: Box::new(api::schema::Request {
                id: "deactivate-surface".into(),
                method: api::schema::Method::ClientShellSurfaceSet(
                    api::schema::ClientShellSurfaceSetParams { active: false },
                ),
            }),
        })
    );
    let _ = control_rx.recv().expect("surface deactivation response");
    assert!(server.clients.contains_key(&client_id));
    let (_, runtime_pane_id) = server.app.parse_pane_id(&surface.panes[0].pane_id).unwrap();
    server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, runtime_pane_id)
        .unwrap()
        .test_process_pty_bytes(b"REACTIVATED");
    assert!(
        server.render_retained_pane_surface_and_stream(&std::collections::HashSet::from([
            runtime_pane_id
        ]))
    );
    assert!(render_rx.try_recv().is_err());

    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id,
            request: Box::new(api::schema::Request {
                id: "reactivate-surface".into(),
                method: api::schema::Method::ClientShellSurfaceSet(
                    api::schema::ClientShellSurfaceSetParams { active: true },
                ),
            }),
        })
    );
    let data = loop {
        let message =
            read_server_message(control_rx.recv().expect("surface reactivation response"));
        match message {
            ServerMessage::ClientShellEndpointResponseChunk {
                request_id, data, ..
            } if request_id == "reactivate-surface" => break data,
            ServerMessage::EndpointControl { .. }
            | ServerMessage::ClientShellEndpointResponseChunk { .. } => continue,
            other => panic!("unexpected surface reactivation message: {other:?}"),
        }
    };
    let reactivation_ack = serde_json::from_slice::<api::schema::SuccessResponse>(&data).unwrap();
    let api::schema::ResponseResult::ClientShellSurfaceSet {
        active: true,
        projection_revision: reactivation_floor,
    } = reactivation_ack.result
    else {
        panic!("expected typed surface reactivation result");
    };
    assert!(reactivation_floor > activation_floor);
    server.render_and_stream();
    let ServerMessage::PaneSurface(surface) =
        read_server_message(render_rx.recv().expect("reactivated surface"))
    else {
        panic!("expected replacement pane surface");
    };
    assert!(frame_text(&surface.frame).contains("REACTIVATED"));
    assert!(surface.projection_revision >= reactivation_floor);
    assert_eq!(surface.surface_revision, 2);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn presentation_sync_epoch_replays_modes_and_title() {
    let mut server = test_headless_server();
    let (writer, control_rx, _render_rx) = test_client_writer();
    let client_id = 63;
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id,
            surface_cols: 80,
            surface_rows: 24,
            cell_width_px: 8,
            cell_height_px: 16,
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: true,
            mouse_capture: true,
            surface_active: true,
            writer,
        })
    );
    let _ = control_rx.recv().expect("initial snapshot");
    server.api_window_title = Some("target title".into());
    {
        let client = server.clients.get_mut(&client_id).unwrap();
        client.host_mouse_capture_active = Some(false);
        client.host_sgr_pixels_active = Some(false);
        client.host_keyboard_report_all_active = Some(false);
    }

    let boot_id = server.client_shell_boot_id.clone();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id,
            request: Box::new(api::schema::Request {
                id: "post-commit-reassert".into(),
                method: api::schema::Method::ClientShellSurfaceSet(
                    api::schema::ClientShellSurfaceSetParams { active: true },
                ),
            }),
        })
    );
    let _ = control_rx
        .recv()
        .expect("typed surface reassertion acknowledgement");
    server.stream_host_mouse_capture_mode();
    server.stream_direct_terminal_keyboard_mode();
    server.sync_window_title();
    assert_eq!(
        server.clients[&client_id].host_mouse_capture_active,
        Some(true),
        "the target mode is sent after, not during, the frozen handoff"
    );
    assert_eq!(
        server.clients[&client_id].host_keyboard_report_all_active,
        Some(false)
    );
    let messages = (0..3)
        .map(|_| read_server_message(control_rx.recv().expect("reassertion effect")))
        .collect::<Vec<_>>();
    assert!(messages
        .iter()
        .any(|message| matches!(message, ServerMessage::MouseCapture { enabled: true, .. })));
    assert!(messages.iter().any(|message| matches!(
        message,
        ServerMessage::ClientShellKeyboardReportAll { enabled: false }
    )));
    assert!(messages.iter().any(|message| matches!(
        message,
        ServerMessage::WindowTitle { title: Some(title) } if title == "target title"
    )));
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn deactivating_last_focused_surface_reports_pane_focus_lost() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"\x1b[?1004h");
    let (control_rx, _render_rx) = connect_matching_test_shell(&mut server, 81);
    let _ = control_rx.recv().expect("initial shell snapshot");
    assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
        client_id: 81,
        focused: true,
    }));
    assert_eq!(
        input_rx.try_recv().expect("initial focus gain"),
        Bytes::from_static(b"\x1b[I")
    );

    assert!(server.set_client_shell_surface_active(81, false).unwrap().0);
    assert!(server.focused_shell_tabs().is_empty());
    let focus_lost = input_rx.try_recv();
    shutdown_test_runtimes(&mut server);
    assert_eq!(
        focus_lost.expect("last focused surface must release pane focus"),
        Bytes::from_static(b"\x1b[O")
    );
}

#[tokio::test]
async fn activating_focused_surface_reports_pane_focus_gained() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"\x1b[?1004h");
    let (control_rx, _render_rx) = connect_matching_test_shell(&mut server, 82);
    let _ = control_rx.recv().expect("initial shell snapshot");
    assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
        client_id: 82,
        focused: true,
    }));
    assert_eq!(
        input_rx.try_recv().expect("initial focus gain"),
        Bytes::from_static(b"\x1b[I")
    );
    assert!(server.set_client_shell_surface_active(82, false).unwrap().0);
    while input_rx.try_recv().is_ok() {}
    assert!(server.focused_shell_tabs().is_empty());
    assert_eq!(server.clients[&82].outer_terminal_focus, Some(true));

    assert!(server.set_client_shell_surface_active(82, true).unwrap().0);
    assert_eq!(server.focused_shell_tabs().len(), 1);
    let focus_gained = input_rx.try_recv();
    shutdown_test_runtimes(&mut server);
    assert_eq!(
        focus_gained.expect("activated focused surface must acquire pane focus"),
        Bytes::from_static(b"\x1b[I")
    );
}
