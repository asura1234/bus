#[test]
fn client_pane_pixel_mouse_uses_runtime_pixel_encoding() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20,
            5,
            0,
            b"\x1b[?1003h\x1b[?1006h\x1b[?1016h",
            4,
        );
    runtime.resize(5, 20, 10, 20);

    apply_client_pane_input_events(
        &runtime,
        &[crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position: crate::protocol::wire::ClientMousePosition::Pixels {
                x: 21,
                y: 22,
                column: 2,
                row: 1,
            },
            geometry: None,
            modifiers: 0,
            lines: 3,
        }],
    )
    .expect("pixel mouse input");
    assert_eq!(
        input_rx.try_recv().expect("encoded pixel mouse"),
        Bytes::from_static(b"\x1b[<35;21;22M")
    );
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn client_pane_pixel_mouse_stays_pixel_scaled_when_sgr_is_reasserted() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[?1003h\x1b[?1006h\x1b[?1016h\x1b[?1006h",
            4,
        );
    runtime.resize(24, 80, 10, 20);

    apply_client_pane_input_events(
        &runtime,
        &[crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Down(
                crate::protocol::wire::ClientMouseButton::Left,
            ),
            position: crate::protocol::wire::ClientMousePosition::Pixels {
                x: 403,
                y: 240,
                column: 40,
                row: 12,
            },
            geometry: None,
            modifiers: 0,
            lines: 1,
        }],
    )
    .expect("pixel mouse input");
    assert_eq!(
        input_rx.try_recv().expect("encoded pixel mouse"),
        Bytes::from_static(b"\x1b[<0;403;240M")
    );
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn client_pane_pixel_mouse_falls_back_to_canonical_cell_position() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20,
            5,
            0,
            b"\x1b[?1003h\x1b[?1006h",
            4,
        );
    runtime.resize(5, 20, 10, 20);

    apply_client_pane_input_events(
        &runtime,
        &[crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position: crate::protocol::wire::ClientMousePosition::Pixels {
                x: 21,
                y: 22,
                column: 2,
                row: 1,
            },
            geometry: None,
            modifiers: 0,
            lines: 3,
        }],
    )
    .expect("cell mouse fallback");
    assert_eq!(
        input_rx.try_recv().expect("encoded cell mouse"),
        Bytes::from_static(b"\x1b[<35;3;2M")
    );
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn client_pane_wheel_input_accumulates_scrollback_offset() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = Vec::new();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20, 5, 4096, &bytes, 4,
        );
    let scroll = |kind| crate::protocol::wire::ClientPaneInputEvent::Mouse {
        kind,
        position: crate::protocol::wire::ClientMousePosition::Cell { column: 2, row: 1 },
        geometry: None,
        modifiers: 0,
        lines: 3,
    };

    apply_client_pane_input_events(
        &runtime,
        &[scroll(crate::protocol::wire::ClientMouseKind::ScrollUp)],
    )
    .expect("first scroll up");
    apply_client_pane_input_events(
        &runtime,
        &[scroll(crate::protocol::wire::ClientMouseKind::ScrollUp)],
    )
    .expect("second scroll up");
    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        6
    );

    apply_client_pane_input_events(
        &runtime,
        &[scroll(crate::protocol::wire::ClientMouseKind::ScrollDown)],
    )
    .expect("scroll down");
    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        3
    );

    runtime.test_process_pty_bytes(b"\x1b[?1003h\x1b[?1006h");
    apply_client_pane_input_events(
        &runtime,
        &[crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position: crate::protocol::wire::ClientMousePosition::Cell { column: 2, row: 1 },
            geometry: None,
            modifiers: 0,
            lines: 3,
        }],
    )
    .expect("reported mouse motion");
    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        3
    );
    assert_eq!(
        input_rx.try_recv().expect("reported mouse motion"),
        Bytes::from_static(b"\x1b[<35;3;2M")
    );

    apply_client_pane_input_events(
        &runtime,
        &[crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Down(
                crate::protocol::wire::ClientMouseButton::Left,
            ),
            position: crate::protocol::wire::ClientMousePosition::Cell { column: 2, row: 1 },
            geometry: None,
            modifiers: 0,
            lines: 3,
        }],
    )
    .expect("mouse button");
    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        0
    );
    assert_eq!(
        input_rx.try_recv().expect("reported mouse button"),
        Bytes::from_static(b"\x1b[<0;3;2M")
    );
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn client_plain_page_keys_scroll_shell_transcript_by_pane_height() {
    with_client_pane_runtime(b"", 0, |runtime, input_rx| {
        apply_client_pane_input_events(
            runtime,
            &[client_page_key(
                crate::protocol::wire::ClientKeyCode::PageUp,
                crossterm::event::KeyModifiers::empty(),
                crate::protocol::wire::ClientKeyKind::Press,
            )],
        )
        .expect("pane PageUp");
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            5
        );

        apply_client_pane_input_events(
            runtime,
            &[client_page_key(
                crate::protocol::wire::ClientKeyCode::PageUp,
                crossterm::event::KeyModifiers::empty(),
                crate::protocol::wire::ClientKeyKind::Release,
            )],
        )
        .expect("pane PageUp release");
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            5
        );

        apply_client_pane_input_events(
            runtime,
            &[client_page_key(
                crate::protocol::wire::ClientKeyCode::PageDown,
                crossterm::event::KeyModifiers::empty(),
                crate::protocol::wire::ClientKeyKind::Press,
            )],
        )
        .expect("pane PageDown");
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            0
        );
        assert!(input_rx.try_recv().is_err(), "page keys reached the shell");
    });
}

#[test]
fn client_page_keys_forward_when_modified_or_owned_by_application() {
    with_client_pane_runtime(b"", 0, |runtime, input_rx| {
        apply_client_pane_input_events(
            runtime,
            &[client_page_key(
                crate::protocol::wire::ClientKeyCode::PageUp,
                crossterm::event::KeyModifiers::CONTROL,
                crate::protocol::wire::ClientKeyKind::Press,
            )],
        )
        .expect("modified pane PageUp");
        assert!(
            input_rx.try_recv().is_ok(),
            "modified PageUp was not forwarded"
        );
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            0
        );
    });

    with_client_pane_runtime(b"\x1b[?1h", 0, |runtime, input_rx| {
        apply_client_pane_input_events(
            runtime,
            &[client_page_key(
                crate::protocol::wire::ClientKeyCode::PageUp,
                crossterm::event::KeyModifiers::empty(),
                crate::protocol::wire::ClientKeyKind::Press,
            )],
        )
        .expect("application PageUp");
        assert_eq!(
            input_rx.try_recv().expect("forwarded application PageUp"),
            Bytes::from_static(b"\x1b[5~")
        );
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            0
        );
    });
}

#[test]
fn client_shell_streams_focused_pane_report_all_demand() {
    with_terminal_session_test_server(|server, terminal_id, _terminal_id_string, _pane_id| {
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
        server.app.state.active = Some(0);
        server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .expect("focused runtime")
            .test_process_pty_bytes(b"\x1b[>15u");

        server.stream_direct_terminal_keyboard_mode();

        assert!(matches!(
            read_server_message(
                client_control_rx
                    .recv_timeout(Duration::from_millis(100))
                    .expect("shell keyboard mode message")
            ),
            ServerMessage::ClientShellKeyboardReportAll { enabled: true }
        ));
    });
}

#[tokio::test]
async fn client_shell_release_cleanup_does_not_promote_and_survives_disconnect() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"\x1b[>3u");
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    for client_id in [1, 2] {
        server.clients.insert(
            client_id,
            ClientConnection::new(
                (80, 24),
                crate::protocol::kitty::HostCellSize::default(),
                client_id,
                unread_test_writer(),
            ),
        );
    }
    let key = |kind| crate::protocol::wire::ClientPaneInputEvent::Key {
        code: crate::protocol::wire::ClientKeyCode::Char('x'),
        modifiers: 0,
        kind,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: (kind == crate::protocol::wire::ClientKeyKind::Press)
            .then(|| "x".to_owned()),
        tracks_release: true,
        physical_key_id: Some(0x2d),
        windows_record: None,
    };

    assert!(
        server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 1,
            pane_id: pane_id.clone(),
            events: vec![key(crate::protocol::wire::ClientKeyKind::Press)],
        })
    );
    assert!(!input_rx.recv().await.expect("encoded press").is_empty());
    assert!(server.promote_client_to_foreground(2));

    assert!(
        !server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 1,
            pane_id: pane_id.clone(),
            events: vec![key(crate::protocol::wire::ClientKeyKind::Release)],
        })
    );
    assert!(!input_rx.recv().await.expect("encoded release").is_empty());
    assert_eq!(server.foreground_client_id, Some(2));

    assert!(
        server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 1,
            pane_id,
            events: vec![key(crate::protocol::wire::ClientKeyKind::Press)],
        })
    );
    assert!(!input_rx
        .recv()
        .await
        .expect("second encoded press")
        .is_empty());
    assert!(server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 1 }));
    assert!(!input_rx
        .recv()
        .await
        .expect("disconnect synthesized release")
        .is_empty());
    shutdown_test_runtimes(&mut server);
}

#[test]
fn client_shell_mouse_capture_combines_local_preference_with_endpoint_demand() {
    let mut server = test_headless_server();
    let (writer, control_rx, _render_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            writer,
        ),
    );

    server.stream_host_mouse_capture_mode();
    assert!(matches!(
        read_server_message(control_rx.recv().expect("initial mouse mode")),
        ServerMessage::MouseCapture {
            enabled: false,
            sgr_pixels: false
        }
    ));
    assert!(
        server.handle_server_event(ServerEvent::ClientShellMouseCapture {
            client_id: 1,
            enabled: true,
        })
    );
    server.stream_host_mouse_capture_mode();
    assert!(matches!(
        read_server_message(control_rx.recv().expect("preferred mouse mode")),
        ServerMessage::MouseCapture {
            enabled: true,
            sgr_pixels: false
        }
    ));
}

#[test]
fn client_shell_focus_promotes_and_reaches_reporting_pane() {
    with_terminal_session_test_server(|server, terminal_id, _other_terminal_id, _pane_id| {
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                24,
                0,
                b"\x1b[?1004h",
                4,
            );
        server
            .app
            .terminal_runtimes
            .insert(terminal_id.clone(), runtime);
        server.app.state.active = Some(0);
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
                (100, 30),
                crate::protocol::kitty::HostCellSize::default(),
                2,
                unread_test_writer(),
            ),
        );
        server.foreground_client_id = Some(2);
        server.sync_foreground_client_state();
        assert!(server.claim_unowned_shell_tab_geometry(2, true));
        assert_eq!(
            server
                .app
                .terminal_runtimes
                .get(&terminal_id)
                .expect("focused runtime")
                .current_size(),
            (30, 99)
        );

        assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
            client_id: 1,
            focused: true,
        }));
        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.app.state.outer_terminal_focus, Some(true));
        assert_eq!(
            server
                .app
                .terminal_runtimes
                .get(&terminal_id)
                .expect("focused runtime")
                .current_size(),
            (24, 79)
        );
        assert_eq!(
            input_rx.try_recv().expect("focus gained input"),
            Bytes::from_static(b"\x1b[I")
        );

        assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
            client_id: 2,
            focused: true,
        }));
        assert!(
            input_rx.try_recv().is_err(),
            "second viewer duplicated focus gain"
        );
        assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
            client_id: 1,
            focused: false,
        }));
        assert!(
            input_rx.try_recv().is_err(),
            "remaining viewer lost tab focus"
        );
        assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
            client_id: 2,
            focused: false,
        }));
        assert_eq!(server.app.state.outer_terminal_focus, Some(false));
        assert_eq!(
            input_rx.try_recv().expect("last viewer focus lost input"),
            Bytes::from_static(b"\x1b[O")
        );
    });
}

#[test]
fn oversized_paste_rejection_notifies_only_the_sending_client() {
    let mut server = test_headless_server();
    let (sender_writer, sender_control_rx, _sender_render_rx) = test_client_writer();
    let (foreground_writer, foreground_control_rx, _foreground_render_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            sender_writer,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            2,
            foreground_writer,
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    assert!(
        !server.handle_server_event(ServerEvent::ClientPasteRejected {
            client_id: 1,
            size: 5_000_012,
            max: 1_048_576,
        })
    );

    match read_server_message(
        sender_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("sending client rejection notification"),
    ) {
        ServerMessage::ClientShellError { message } => assert_eq!(
            message,
            "Paste rejected: Input message is 5000012 bytes; Herdr's limit is 1048576 bytes"
        ),
        other => panic!("expected client shell paste error, got {other:?}"),
    }
    let (shell_writer, shell_control_rx, _shell_render_rx) = test_client_writer();
    server.clients.insert(
        3,
        ClientConnection::new(
            (100, 30),
            crate::protocol::kitty::HostCellSize::default(),
            3,
            shell_writer,
        ),
    );
    assert!(
        !server.handle_server_event(ServerEvent::ClientPasteRejected {
            client_id: 3,
            size: 7_000_000,
            max: 1_048_576,
        })
    );
    match read_server_message(
        shell_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("client shell rejection error"),
    ) {
        ServerMessage::ClientShellError { message } => assert_eq!(
            message,
            "Paste rejected: Input message is 7000000 bytes; Herdr's limit is 1048576 bytes"
        ),
        other => panic!("expected client shell paste error, got {other:?}"),
    }
    assert!(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "foreground client must not receive another client's rejection"
    );
    assert_eq!(server.foreground_client_id, Some(2));
    assert_eq!(server.clients.len(), 3);
    assert!(server.app.state.toast.is_none());
}
