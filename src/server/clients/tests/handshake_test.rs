use super::*;
use interprocess::local_socket::traits::Listener as _;
use std::path::PathBuf;

struct TestSocketPath(PathBuf);

impl Drop for TestSocketPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn unique_test_path(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let filename = format!("h{}-{nanos}.sock", std::process::id());
    #[cfg(unix)]
    {
        let _ = name;
        PathBuf::from("/tmp").join(filename)
    }
    #[cfg(windows)]
    {
        std::env::temp_dir().join(format!("herdr-{name}-{filename}"))
    }
}

fn local_stream_pair(name: &str) -> (LocalStream, LocalStream, TestSocketPath) {
    let path = unique_test_path(name);
    let _ = std::fs::remove_file(&path);
    let listener = crate::ipc::bind_local_listener(&path).unwrap();
    let client = crate::ipc::connect_local_stream(&path).unwrap();
    let server = listener.accept().unwrap();
    (client, server, TestSocketPath(path))
}

fn start_client_read_loop(
    name: &str,
) -> (
    LocalStream,
    mpsc::Receiver<ServerEvent>,
    Arc<AtomicBool>,
    std::thread::JoinHandle<()>,
    TestSocketPath,
) {
    let (client_stream, server_stream, path) = local_stream_pair(name);
    let (server_event_tx, server_event_rx) = mpsc::channel(4);
    let should_quit = Arc::new(AtomicBool::new(false));
    let read_quit = should_quit.clone();
    let handle = std::thread::spawn(move || {
        client_read_loop(server_stream, 7, &server_event_tx, &read_quit)
    });
    (client_stream, server_event_rx, should_quit, handle, path)
}

fn endpoint_hello(surface_cols: u16, surface_rows: u16) -> ClientMessage {
    let hello = EndpointClientHello {
        generation: ENDPOINT_PROTOCOL_GENERATION,
        client_version: crate::build_info::version(),
        cell_width_px: 8,
        cell_height_px: 16,
        surface_size: crate::protocol::ClientSurfaceSize {
            cols: surface_cols,
            rows: surface_rows,
        },
        pixel_mouse: true,
        direct_graphics: true,
        endpoint_keybindings: true,
        mouse_capture: true,
        surface_active: true,
        snapshot_codecs: vec![crate::protocol::endpoint::SNAPSHOT_CODEC_V1.into()],
        surface_codecs: vec![crate::protocol::endpoint::SURFACE_CODEC_V1.into()],
        input_codecs: vec![crate::protocol::endpoint::INPUT_CODEC_V1.into()],
        blob_codecs: vec![crate::protocol::endpoint::BLOB_CODEC_V1.into()],
    };
    ClientMessage::EndpointControl {
        kind: ENDPOINT_HELLO_KIND.into(),
        data: serde_json::to_string(&hello).unwrap(),
    }
}

fn endpoint_welcome(message: ServerMessage) -> EndpointServerWelcome {
    let ServerMessage::EndpointControl { kind, data } = message else {
        panic!("expected endpoint welcome");
    };
    assert_eq!(kind, ENDPOINT_WELCOME_KIND);
    serde_json::from_str(&data).unwrap()
}

fn rejected_handshake(message: ClientMessage) -> EndpointServerWelcome {
    let (mut client_stream, server_stream, _path) = local_stream_pair("rejected-handshake");
    let (server_event_tx, mut server_event_rx) = mpsc::channel(4);
    let should_quit = Arc::new(AtomicBool::new(false));
    let handle = std::thread::spawn(move || {
        handle_client_handshake(server_stream, 43, &server_event_tx, &should_quit)
    });
    protocol::write_message(&mut client_stream, &message).expect("write rejected hello");
    let welcome = endpoint_welcome(
        protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).expect("read rejection"),
    );
    handle.join().unwrap().unwrap();
    assert!(matches!(
        server_event_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    let next: Result<ServerMessage, _> = protocol::read_message(&mut client_stream, MAX_FRAME_SIZE);
    assert!(matches!(next, Err(protocol::FramingError::UnexpectedEof)));
    welcome
}

fn recv_server_event(receiver: &mut mpsc::Receiver<ServerEvent>, context: &str) -> ServerEvent {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match receiver.try_recv() {
            Ok(event) => return event,
            Err(mpsc::error::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(err) => panic!("{context}: {err}"),
        }
    }
}

fn test_queue_writer() -> (ClientWriter, Arc<ClientWriterQueue>) {
    let queue = ClientWriterQueue::new();
    (
        ClientWriter {
            control: ClientControlWriter::queue(queue.clone()),
            render: ClientRenderWriter::queue(queue.clone()),
        },
        queue,
    )
}

fn frame_server_message(message: &ServerMessage) -> Vec<u8> {
    let mut bytes = Vec::new();
    protocol::write_message(&mut bytes, message).expect("frame server message");
    bytes
}

#[path = "read_loop_test.rs"]
mod read_loop;

#[path = "writer_test.rs"]
mod writer;

#[test]
fn handshake_rejects_different_builds_and_names_both_versions() {
    let server_version = crate::build_info::version();
    for client_version in ["older-build", "newer-build", ""] {
        let ClientMessage::EndpointControl { kind, data } = endpoint_hello(80, 24) else {
            unreachable!();
        };
        let mut hello: serde_json::Value = serde_json::from_str(&data).unwrap();
        hello["client_version"] = client_version.into();
        let welcome = rejected_handshake(ClientMessage::EndpointControl {
            kind,
            data: hello.to_string(),
        });
        assert_eq!(welcome.server_version, server_version);
        let error = welcome.error.expect("different build rejected");
        assert_eq!(error.code, "build_mismatch");
        assert!(error
            .message
            .contains(&format!("client build {client_version:?}")));
        assert!(error
            .message
            .contains(&format!("server build {server_version:?}")));
    }
}

#[test]
fn handshake_requires_a_client_version_without_a_fallback() {
    let ClientMessage::EndpointControl { kind, data } = endpoint_hello(80, 24) else {
        unreachable!();
    };
    let mut hello: serde_json::Value = serde_json::from_str(&data).unwrap();
    hello.as_object_mut().unwrap().remove("client_version");
    let welcome = rejected_handshake(ClientMessage::EndpointControl {
        kind,
        data: hello.to_string(),
    });
    let error = welcome.error.expect("missing build rejected");
    assert_eq!(error.code, "invalid_hello");
    assert!(error.message.contains("missing field `client_version`"));
}

#[test]
fn handshake_rejects_direct_terminal_hello() {
    let welcome = rejected_handshake(ClientMessage::TerminalHello {
        version: crate::protocol::PROTOCOL_VERSION,
        cols: 80,
        rows: 24,
        cell_width_px: 8,
        cell_height_px: 16,
        pixel_mouse: false,
    });
    assert_eq!(welcome.error.unwrap().code, "invalid_hello");
}

#[test]
fn client_shell_geometry_rejects_unsafe_dimensions_and_cell_sizes() {
    assert!(client_shell_geometry_error(
        crate::protocol::ClientSurfaceSize { cols: 80, rows: 24 },
        8,
        16,
    )
    .is_none());
    assert!(client_shell_geometry_error(
        crate::protocol::ClientSurfaceSize {
            cols: MAX_CLIENT_SHELL_DIMENSION,
            rows: MAX_CLIENT_SHELL_DIMENSION,
        },
        8,
        16,
    )
    .is_some());
    assert!(client_shell_geometry_error(
        crate::protocol::ClientSurfaceSize { cols: 80, rows: 24 },
        MAX_CLIENT_CELL_SIZE_PX + 1,
        16,
    )
    .is_some());
}

#[test]
fn dedicated_client_shell_handshake_uses_surface_viewport() {
    let (mut client_stream, server_stream, _path) = local_stream_pair("client-shell-handshake");
    let (server_event_tx, mut server_event_rx) = mpsc::channel(4);
    let should_quit = Arc::new(AtomicBool::new(false));
    let handshake_quit = should_quit.clone();
    let handle = std::thread::spawn(move || {
        handle_client_handshake(server_stream, 43, &server_event_tx, &handshake_quit)
    });

    protocol::write_message(&mut client_stream, &endpoint_hello(80, 29))
        .expect("write shell hello");

    let welcome: ServerMessage =
        protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).expect("read welcome");
    let welcome = endpoint_welcome(welcome);
    assert_eq!(welcome.generation, ENDPOINT_PROTOCOL_GENERATION);
    assert_eq!(welcome.server_version, crate::build_info::version());
    assert!(welcome.error.is_none());
    match server_event_rx
        .blocking_recv()
        .expect("client shell connected event")
    {
        ServerEvent::ClientShellConnected {
            client_id,
            surface_cols,
            surface_rows,
            cell_width_px,
            cell_height_px,
            pixel_mouse,
            direct_graphics,
            endpoint_keybindings,
            mouse_capture,
            surface_active,
            writer,
        } => {
            assert_eq!(client_id, 43);
            assert_eq!((surface_cols, surface_rows), (80, 29));
            assert_eq!((cell_width_px, cell_height_px), (8, 16));
            assert!(pixel_mouse);
            assert!(direct_graphics);
            assert!(endpoint_keybindings);
            assert!(mouse_capture);
            assert!(surface_active);
            drop(writer);
        }
        other => panic!("expected ClientShellConnected, got {other:?}"),
    }

    drop(client_stream);
    should_quit.store(true, Ordering::Release);
    handle
        .join()
        .expect("handshake thread join")
        .expect("handshake thread result");
}

#[test]
fn dedicated_client_shell_handshake_rejects_empty_surface() {
    let (mut client_stream, server_stream, _path) = local_stream_pair("client-shell-empty-surface");
    let (server_event_tx, mut server_event_rx) = mpsc::channel(4);
    let should_quit = Arc::new(AtomicBool::new(false));
    let handshake_quit = should_quit.clone();
    let handle = std::thread::spawn(move || {
        handle_client_handshake(server_stream, 43, &server_event_tx, &handshake_quit)
    });

    protocol::write_message(&mut client_stream, &endpoint_hello(0, 29))
        .expect("write empty shell hello");

    let welcome: ServerMessage =
        protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).expect("read welcome");
    let welcome = endpoint_welcome(welcome);
    assert!(welcome
        .error
        .is_some_and(|error| error.message.contains("non-empty pane surface")));
    handle
        .join()
        .expect("handshake thread join")
        .expect("handshake thread result");
    assert!(server_event_rx.try_recv().is_err());
}

#[test]
fn handshake_timeout_is_within_five_second_deadline() {
    // The handshake timeout must be short enough that
    // the connection is guaranteed to close within 5 seconds even with
    // OS overhead (thread scheduling, timer slack, cleanup).
    assert!(
        HANDSHAKE_TIMEOUT < Duration::from_secs(5),
        "HANDSHAKE_TIMEOUT ({:?}) must be less than 5 seconds to guarantee \
             connection close within the 5-second deadline",
        HANDSHAKE_TIMEOUT
    );
}
