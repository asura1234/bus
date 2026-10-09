use super::*;

#[test]
fn unknown_endpoint_method_returns_correlated_error() {
    let decoded =
        decode_endpoint_request(r#"{"id":"req-1","method":"plugin.future","params":{"value":1}}"#)
            .unwrap();
    assert!(matches!(
        decoded,
        DecodedEndpointRequest::Error {
            request_id,
            code: "unsupported_method",
            ..
        } if request_id == "req-1"
    ));
}

#[test]
fn malformed_known_endpoint_method_returns_correlated_error() {
    let decoded =
        decode_endpoint_request(r#"{"id":"req-2","method":"workspace.focus","params":{}}"#)
            .unwrap();
    assert!(matches!(
        decoded,
        DecodedEndpointRequest::Error {
            request_id,
            code: "invalid_request",
            ..
        } if request_id == "req-2"
    ));
}

#[test]
fn connected_shell_does_not_reply_to_retired_health_ping() {
    let (mut client_stream, server_stream, _path) = local_stream_pair("client-no-pong");
    let (server_event_tx, mut server_event_rx) = mpsc::channel(4);
    let should_quit = Arc::new(AtomicBool::new(false));
    let handle = std::thread::spawn(move || {
        handle_client_handshake(server_stream, 43, &server_event_tx, &should_quit)
    });
    protocol::write_message(&mut client_stream, &endpoint_hello(80, 24)).unwrap();
    let welcome =
        endpoint_welcome(protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).unwrap());
    assert!(welcome.error.is_none());
    let ServerEvent::ClientShellConnected { writer, .. } =
        recv_server_event(&mut server_event_rx, "shell connection")
    else {
        panic!("expected shell connection");
    };
    for (kind, data) in [
        ("endpoint.health.ping.v1", "probe"),
        (
            crate::protocol::wire::handshake::PRESENTATION_EFFECTS_SYNC_KIND,
            "barrier",
        ),
    ] {
        protocol::write_message(
            &mut client_stream,
            &ClientMessage::EndpointControl {
                kind: kind.into(),
                data: data.into(),
            },
        )
        .unwrap();
    }
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "control barrier"),
        ServerEvent::ClientShellPresentationSync { client_id: 43, token }
            if token == "barrier"
    ));
    let marker = ServerMessage::ClientShellError {
        message: "control queue barrier".into(),
    };
    writer.control.send(frame_server_message(&marker)).unwrap();
    let reply: ServerMessage = protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).unwrap();
    assert!(matches!(
        reply,
        ServerMessage::ClientShellError { message } if message == "control queue barrier"
    ));
    protocol::write_message(&mut client_stream, &ClientMessage::Detach).unwrap();
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "shell detach"),
        ServerEvent::ClientDetach { client_id: 43 }
    ));
    handle.join().unwrap().unwrap();
}

#[test]
fn client_read_loop_stops_after_detach() {
    let (mut client_stream, mut server_event_rx, _should_quit, handle, _path) =
        start_client_read_loop("client-read-detach");

    let mut messages = Vec::new();
    protocol::write_message(&mut messages, &ClientMessage::Detach).unwrap();
    protocol::write_message(
        &mut messages,
        &ClientMessage::ClipboardImage {
            target: crate::protocol::wire::ClientClipboardImageTarget::Pane("w1:p1".into()),
            extension: "png".into(),
            data: vec![1, 2, 3],
        },
    )
    .unwrap();
    client_stream
        .write_all(&messages)
        .expect("write detach and trailing message");

    assert!(matches!(
        recv_server_event(&mut server_event_rx, "detach event"),
        ServerEvent::ClientDetach { client_id: 7 }
    ));
    handle.join().expect("read thread join");
    assert!(server_event_rx.try_recv().is_err());
}

#[test]
fn client_read_loop_ignores_unknown_controls_including_retired_health_ping() {
    let (mut client_stream, mut server_event_rx, _should_quit, handle, _path) =
        start_client_read_loop("client-read-future-control");

    for kind in ["future.optional.v1", "endpoint.health.ping.v1"] {
        protocol::write_message(
            &mut client_stream,
            &ClientMessage::EndpointControl {
                kind: kind.into(),
                data: "{}".into(),
            },
        )
        .unwrap();
    }
    protocol::write_message(&mut client_stream, &ClientMessage::Detach).unwrap();

    assert!(matches!(
        recv_server_event(&mut server_event_rx, "detach after future control"),
        ServerEvent::ClientDetach { client_id: 7 }
    ));
    handle.join().expect("read thread join");
}

#[test]
fn client_read_loop_rejects_oversized_shell_paste_without_disconnect() {
    let (mut client_stream, mut server_event_rx, should_quit, handle, _path) =
        start_client_read_loop("client-read-shell-paste");
    for size in [MAX_INPUT_PAYLOAD, MAX_INPUT_PAYLOAD + 1, 1] {
        protocol::write_message(
            &mut client_stream,
            &ClientMessage::ClientShellPaneInput {
                pane_id: "w1:p1".into(),
                events: vec![ClientPaneInputEvent::Paste("x".repeat(size))],
            },
        )
        .unwrap();
        match recv_server_event(&mut server_event_rx, "shell paste result") {
            ServerEvent::ClientShellPaneInput {
                client_id, events, ..
            } => {
                assert_eq!(client_id, 7);
                assert!(size <= MAX_INPUT_PAYLOAD);
                assert!(
                    matches!(&events[..], [ClientPaneInputEvent::Paste(text)] if text.len() == size)
                );
            }
            ServerEvent::ClientPasteRejected {
                client_id,
                size: rejected,
                max,
            } => {
                assert_eq!(client_id, 7);
                assert_eq!(size, MAX_INPUT_PAYLOAD + 1);
                assert_eq!(rejected, size);
                assert_eq!(max, MAX_INPUT_PAYLOAD);
            }
            other => panic!("unexpected shell paste result: {other:?}"),
        }
    }
    drop(client_stream);
    should_quit.store(true, Ordering::Release);
    handle.join().unwrap();
}

#[test]
fn client_read_loop_disconnects_oversized_shell_text() {
    let (mut client_stream, mut server_event_rx, _should_quit, handle, _path) =
        start_client_read_loop("client-read-shell-text");
    protocol::write_message(
        &mut client_stream,
        &ClientMessage::ClientShellPaneInput {
            pane_id: "w1:p1".into(),
            events: vec![ClientPaneInputEvent::TextCommit(
                "x".repeat(MAX_INPUT_PAYLOAD + 1),
            )],
        },
    )
    .unwrap();
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "oversized text disconnect"),
        ServerEvent::ClientDisconnected { client_id: 7 }
    ));
    handle.join().unwrap();
}

#[test]
fn client_read_loop_closes_on_unsafe_shell_resize() {
    let (mut client_stream, mut server_event_rx, _should_quit, handle, _path) =
        start_client_read_loop("client-read-unsafe-resize");

    protocol::write_message(
        &mut client_stream,
        &ClientMessage::ClientShellResize {
            cell_width_px: 8,
            cell_height_px: 16,
            surface_size: crate::protocol::wire::ClientSurfaceSize {
                cols: MAX_CLIENT_SHELL_DIMENSION,
                rows: MAX_CLIENT_SHELL_DIMENSION,
            },
            pixel_mouse: false,
        },
    )
    .unwrap();

    assert!(matches!(
        recv_server_event(&mut server_event_rx, "unsafe resize disconnect"),
        ServerEvent::ClientDisconnected { client_id: 7 }
    ));
    handle.join().expect("read thread join");
}

#[test]
fn client_read_loop_uses_authoritative_shell_resize_surface() {
    let (mut client_stream, mut server_event_rx, _should_quit, handle, _path) =
        start_client_read_loop("client-read-resize");

    protocol::write_message(
        &mut client_stream,
        &ClientMessage::ClientShellResize {
            cell_width_px: 8,
            cell_height_px: 16,
            surface_size: crate::protocol::wire::ClientSurfaceSize { cols: 60, rows: 15 },
            pixel_mouse: true,
        },
    )
    .expect("write shell resize");
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "shell resize"),
        ServerEvent::ClientShellResize {
            client_id: 7,
            surface_cols: 60,
            surface_rows: 15,
            cell_width_px: 8,
            cell_height_px: 16,
            pixel_mouse: true,
        }
    ));

    protocol::write_message(&mut client_stream, &ClientMessage::Detach).expect("write detach");
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "detach event"),
        ServerEvent::ClientDetach { client_id: 7 }
    ));
    handle.join().expect("read thread join");
}

#[test]
fn client_read_loop_keeps_single_host_theme_updates_ordered_and_palette_bounded() {
    let (mut client_stream, mut server_event_rx, should_quit, handle, _path) =
        start_client_read_loop("client-read-host-theme");

    let colors = (0..=u8::MAX)
        .map(|index| {
            (
                index,
                crate::protocol::wire::ClientHostColor {
                    r: index,
                    g: 0,
                    b: 0,
                },
            )
        })
        .collect();
    protocol::write_message(
        &mut client_stream,
        &ClientMessage::ClientShellHostTheme {
            update: crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(colors),
        },
    )
    .expect("write bounded palette update");
    protocol::write_message(
        &mut client_stream,
        &ClientMessage::ClientShellHostTheme {
            update: crate::protocol::wire::ClientHostThemeUpdate::Appearance(
                crate::protocol::wire::ClientHostAppearance::Dark,
            ),
        },
    )
    .expect("write ordered appearance update");

    assert!(matches!(
        recv_server_event(&mut server_event_rx, "bounded palette update"),
        ServerEvent::ClientShellHostTheme {
            client_id: 7,
            update: crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(colors),
        } if colors.len() == 256
    ));
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "ordered appearance update"),
        ServerEvent::ClientShellHostTheme {
            client_id: 7,
            update: crate::protocol::wire::ClientHostThemeUpdate::Appearance(
                crate::protocol::wire::ClientHostAppearance::Dark
            ),
        }
    ));

    let colors = vec![
        (
            0,
            crate::protocol::wire::ClientHostColor { r: 0, g: 0, b: 0 },
        );
        257
    ];
    protocol::write_message(
        &mut client_stream,
        &ClientMessage::ClientShellHostTheme {
            update: crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(colors),
        },
    )
    .expect("write oversized palette update");
    assert!(matches!(
        recv_server_event(&mut server_event_rx, "oversized palette disconnect"),
        ServerEvent::ClientDisconnected { client_id: 7 }
    ));

    drop(client_stream);
    should_quit.store(true, Ordering::Release);
    handle.join().expect("read thread join");
}

#[test]
fn pane_input_limits_charge_scroll_repeats() {
    let oversized_scroll = ClientPaneInputEvent::Mouse {
        kind: crate::protocol::wire::ClientMouseKind::ScrollUp,
        position: crate::protocol::wire::ClientMousePosition::Cell { column: 0, row: 0 },
        geometry: None,
        modifiers: 0,
        lines: (MAX_INPUT_EVENT_BATCH + 1) as u16,
    };
    assert_eq!(
        pane_input_event_limit(&[oversized_scroll]),
        InputEventLimit::TooManyEvents
    );
}
