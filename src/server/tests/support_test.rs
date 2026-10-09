use super::*;

pub(super) fn client_shell_snapshot(
    message: ServerMessage,
) -> Box<crate::protocol::wire::ClientShellSnapshot> {
    let ServerMessage::EndpointControl { kind, data } = message else {
        panic!("expected client shell snapshot");
    };
    assert_eq!(
        kind,
        crate::protocol::wire::handshake::ENDPOINT_SNAPSHOT_KIND
    );
    Box::new(serde_json::from_str(&data).expect("decode client shell snapshot"))
}

pub(super) fn test_headless_server() -> HeadlessServer {
    test_headless_server_with_event_hub(crate::server::api::EventHub::default())
}

pub(super) fn test_headless_server_with_event_hub(
    event_hub: crate::server::api::EventHub,
) -> HeadlessServer {
    let config = crate::utils::config::Config::default();
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::server::app::App::new(
        &config,
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        event_hub,
    );

    app.state.default_shell = crate::server::api::test_support::exiting_test_command().into();
    // Parallel harnesses can read the same microsecond clock; the counter keeps their sockets apart.
    static NEXT_DIR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hh-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        NEXT_DIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::create_dir_all(&dir);
    let socket_path = dir.join("client.sock");
    let _ = fs::remove_file(&socket_path);
    let listener = bind_local_listener(&socket_path).expect("bind test listener");
    let client_socket_identity =
        socket_file_identity(&socket_path).expect("test listener socket identity");
    #[cfg(unix)]
    listener
        .set_nonblocking(ListenerNonblockingMode::Accept)
        .expect("set listener nonblocking");
    let (server_event_tx, server_event_rx) = mpsc::channel(64);
    let should_quit = Arc::new(AtomicBool::new(false));
    #[cfg(windows)]
    spawn_windows_client_accept_thread(listener, should_quit.clone(), server_event_tx.clone());
    let headless_size = app.state.headless_size;

    HeadlessServer {
        app,
        _api_tx: None,
        _api_server: None,
        #[cfg(unix)]
        client_listener: listener,
        client_socket_path: socket_path,
        client_socket_identity,
        clients: HashMap::new(),
        #[cfg(unix)]
        next_client_id: 1,
        foreground_client_id: None,
        tab_geometry_controllers: HashMap::new(),
        client_shell_boot_id: "test-boot".into(),
        sent_window_title: None,
        api_window_title: None,
        server_config_diagnostic: None,
        pending_alt_screen_reads: Vec::new(),
        deferred_alt_screen_reads: Vec::new(),
        next_activity_stamp: 1,
        headless_size,
        effective_size: headless_size,
        shutting_down: false,
        should_quit,
        server_event_rx,
        server_event_tx,
    }
}

pub(super) fn shutdown_test_runtimes(server: &mut HeadlessServer) {
    for (_, runtime) in server.app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

pub(super) fn read_server_message(bytes: Vec<u8>) -> ServerMessage {
    let mut cursor = std::io::Cursor::new(bytes);
    protocol::read_message(&mut cursor, MAX_FRAME_SIZE).expect("decode server message")
}

pub(super) fn frame_text(frame: &FrameData) -> String {
    frame
        .cells
        .chunks(usize::from(frame.width))
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn headless_pane_list(server: &mut HeadlessServer) -> Vec<api::schema::PaneInfo> {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
        request: api::schema::Request {
            id: "list-titles".into(),
            method: api::schema::Method::PaneList(api::schema::PaneListParams::default()),
        },
        respond_to,
    });
    let response: api::schema::SuccessResponse =
        serde_json::from_str(&response_rx.recv().unwrap()).unwrap();
    let api::schema::ResponseResult::PaneList { panes } = response.result else {
        panic!("expected pane list");
    };
    panes
}

pub(super) fn pane_updated_events(event_hub: &crate::server::api::EventHub) -> usize {
    event_hub
        .events_after(0)
        .iter()
        .filter(|(_, event)| event.event == api::schema::EventKind::PaneUpdated)
        .count()
}

pub(super) fn window_title_test_server() -> (HeadlessServer, std::sync::mpsc::Receiver<Vec<u8>>) {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::server::workspaces::Workspace::test_new("herd")];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;

    let (client_tx, control_rx, _render_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            client_tx,
        ),
    );
    server.promote_client_to_foreground(1);
    drain_window_titles(&control_rx);
    (server, control_rx)
}

/// The test client writer drains its queue on a background thread, so
/// reading a pushed message needs a timeout rather than `try_recv`.
pub(super) fn next_window_title(
    control_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
) -> Option<Option<String>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(bytes) = control_rx.recv_timeout(remaining) else {
            return None;
        };
        if let ServerMessage::WindowTitle { title } = read_server_message(bytes) {
            return Some(title);
        }
    }
    None
}

pub(super) fn drain_window_titles(control_rx: &std::sync::mpsc::Receiver<Vec<u8>>) {
    while control_rx.recv_timeout(Duration::from_millis(50)).is_ok() {}
}

pub(super) fn no_window_title(control_rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> bool {
    while let Ok(bytes) = control_rx.recv_timeout(Duration::from_millis(200)) {
        if let ServerMessage::WindowTitle { .. } = read_server_message(bytes) {
            return false;
        }
    }
    true
}

/// A writer for clients whose output the test never reads; its channels stay
/// open so sends succeed as they do for a live client.
pub(super) fn unread_test_writer() -> ClientWriter {
    let (writer, control_rx, render_rx) = test_client_writer();
    std::mem::forget((control_rx, render_rx));
    writer
}

pub(super) fn test_client_writer() -> (
    ClientWriter,
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    let (render_tx, render_rx) = std::sync::mpsc::sync_channel(1);
    (
        ClientWriter::test_channel(control_tx, render_tx),
        control_rx,
        render_rx,
    )
}

pub(super) fn install_shared_view_test_runtime(
    server: &mut HeadlessServer,
) -> crate::utils::ids::PaneId {
    let mut workspace = crate::server::workspaces::Workspace::test_new("shared-view");
    let pane_id = workspace.focused_pane_id().expect("focused pane");
    workspace.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 23, b"BASE"),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    pane_id
}

pub(super) fn connect_test_shell(
    server: &mut HeadlessServer,
    client_id: u64,
    surface_cols: u16,
    surface_rows: u16,
) -> (
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (writer, control, render) = test_client_writer();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            client_id,
            surface_cols,
            surface_rows,
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
    (control, render)
}

pub(super) fn connect_matching_test_shell(
    server: &mut HeadlessServer,
    client_id: u64,
) -> (
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    connect_test_shell(server, client_id, 80, 23)
}

pub(super) fn write_shared_test_pane(
    server: &mut HeadlessServer,
    pane_id: crate::utils::ids::PaneId,
    bytes: &[u8],
) {
    server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("pane runtime")
        .test_process_pty_bytes(bytes);
}

pub(super) fn recv_pane_surface(
    receiver: &std::sync::mpsc::Receiver<Vec<u8>>,
    context: &str,
) -> crate::protocol::wire::PaneSurfaceFrame {
    match read_server_message(
        receiver
            .recv()
            .unwrap_or_else(|error| panic!("{context}: {error}")),
    ) {
        ServerMessage::PaneSurface(surface) => surface,
        other => panic!("{context}: expected pane surface, got {other:?}"),
    }
}

pub(super) fn recv_pane_surface_patch(
    receiver: &std::sync::mpsc::Receiver<Vec<u8>>,
    context: &str,
) -> crate::protocol::wire::PaneSurfacePatch {
    match read_server_message(
        receiver
            .recv()
            .unwrap_or_else(|error| panic!("{context}: {error}")),
    ) {
        ServerMessage::PaneSurfacePatch(patch) => patch,
        other => panic!("{context}: expected pane surface patch, got {other:?}"),
    }
}

pub(super) fn install_focused_test_runtime(
    server: &mut HeadlessServer,
    terminal_bytes: &[u8],
) -> tokio::sync::mpsc::Receiver<Bytes> {
    let mut workspace = crate::server::workspaces::Workspace::test_new("focus-reporting");
    let pane_id = workspace.tabs[0].root_pane;
    let (runtime, input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            terminal_bytes,
            4,
        );
    workspace.insert_test_runtime(pane_id, runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    input_rx
}

pub(super) fn with_terminal_session_test_server(
    test: impl FnOnce(&mut HeadlessServer, crate::utils::ids::TerminalId, String, String),
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut server = test_headless_server();
    let workspace = crate::server::workspaces::Workspace::test_new("test");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).expect("terminal id").clone();
    let terminal_id_string = terminal_id.to_string();
    let public_pane_id = format!("{}:p1", workspace.id);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.terminal_runtimes.insert(
        terminal_id.clone(),
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );

    test(&mut server, terminal_id, terminal_id_string, public_pane_id);

    drop(server);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

pub(super) fn with_client_pane_runtime(
    initial_bytes: &[u8],
    initial_scroll: usize,
    test: impl FnOnce(&crate::terminal::TerminalRuntime, &mut mpsc::Receiver<Bytes>),
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = initial_bytes.to_vec();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20, 5, 4096, &bytes, 4,
        );
    if initial_scroll > 0 {
        runtime.scroll_up(initial_scroll);
    }

    test(&runtime, &mut input_rx);

    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

pub(super) fn client_page_key(
    code: crate::protocol::wire::ClientKeyCode,
    modifiers: crossterm::event::KeyModifiers,
    kind: crate::protocol::wire::ClientKeyKind,
) -> crate::protocol::wire::ClientPaneInputEvent {
    crate::protocol::wire::ClientPaneInputEvent::Key {
        code,
        modifiers: modifiers.bits(),
        kind,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: None,
        windows_record: None,
    }
}
