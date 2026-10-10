use super::limit_snapshot_lines;

#[test]
fn line_limit_preserves_endings_and_reports_omitted_lines() {
    let snapshot = limit_snapshot_lines("one\ntwø\n三\n".into(), Some(2));
    assert_eq!(snapshot.text, "twø\n三\n");
    assert!(snapshot.truncated);

    let snapshot = limit_snapshot_lines("one\ntwo\nthree".into(), Some(1));
    assert_eq!(snapshot.text, "three");
    assert!(snapshot.truncated);

    let snapshot = limit_snapshot_lines("one\ntwo".into(), Some(0));
    assert_eq!(snapshot.text, "");
    assert!(snapshot.truncated);

    let snapshot = limit_snapshot_lines("".into(), Some(2));
    assert_eq!(snapshot.text, "");
    assert!(!snapshot.truncated);
}

#[test]
fn omitted_line_limit_returns_the_complete_snapshot() {
    let snapshot = limit_snapshot_lines("one\ntwo\n".into(), None);
    assert_eq!(snapshot.text, "one\ntwo\n");
    assert!(!snapshot.truncated);
}

#[tokio::test]
async fn recent_read_honors_line_requests_above_one_thousand() {
    let terminal =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(80, 3, 10_000_000, &[]);
    for index in 0..1500 {
        terminal.test_process_pty_bytes(format!("{index:06}\r\n").as_bytes());
    }
    let snapshot = super::read_terminal_snapshot(
        &terminal,
        crate::protocol::api::schema::ReadSource::Recent,
        crate::protocol::api::schema::ReadFormat::Text,
        Some(5000),
    );
    let returned = snapshot
        .text
        .split_inclusive('\n')
        .filter(|line| !line.is_empty())
        .count();
    assert!(
        returned > 1000,
        "expected more than the old 1000-line clamp, got {returned}"
    );
    assert!(
        snapshot.text.contains("000000"),
        "honored 5000-line window should include the oldest retained row"
    );
    assert!(
        !snapshot.truncated,
        "fewer rows than requested means available history is exhausted"
    );
}

#[tokio::test]
async fn room_orchestrator_core_recent_read_reports_exact_range_facts() {
    let terminal =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(12, 4, 1_000_000, &[]);
    for index in 0..30 {
        terminal.test_process_pty_bytes(format!("row-{index:02}\r\n").as_bytes());
    }
    let snapshot = super::read_terminal_snapshot(
        &terminal,
        crate::protocol::api::schema::ReadSource::Recent,
        crate::protocol::api::schema::ReadFormat::Text,
        Some(17),
    );
    assert_eq!(snapshot.requested_lines, Some(17));
    assert_eq!(snapshot.returned_lines, 17);
    assert!(snapshot
        .available_lines
        .is_some_and(|available| available >= 17));
    assert_eq!(snapshot.exhausted, Some(false));
    assert!(snapshot.revision > 0);
}

#[tokio::test]
async fn room_orchestrator_core_visible_read_reports_complete_viewport_facts() {
    let terminal =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(12, 4, 1_000_000, b"a\r\nb");
    let snapshot = super::read_terminal_snapshot(
        &terminal,
        crate::protocol::api::schema::ReadSource::Visible,
        crate::protocol::api::schema::ReadFormat::Text,
        None,
    );
    assert_eq!(snapshot.viewport_rows, Some(4));
    assert_eq!(snapshot.viewport_columns, Some(12));
    assert_eq!(snapshot.requested_lines, None);
    assert_eq!(snapshot.available_lines, None);
    assert_eq!(snapshot.exhausted, None);
    assert!(!snapshot.truncated);
}

#[cfg(unix)]
#[tokio::test]
async fn frozen_visible_read_keeps_range_metadata_during_partial_redraw() {
    use crate::protocol::api::schema::{
        Method, PaneReadParams, ReadFormat, ReadIntent, ReadSource, Request, ResponseResult,
        SuccessResponse,
    };
    use crate::server::main_loop::HeadlessServer;
    use std::collections::HashMap;
    use std::sync::{atomic::AtomicBool, Arc};

    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::server::app::App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![crate::server::workspaces::Workspace::test_new(
        "frozen read",
    )];
    app.state.ensure_test_terminals();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(
            Some(crate::agents::AgentKind::Codex),
            crate::agents::AgentState::Idle,
        );
    let runtime = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
        20,
        5,
        1_000_000,
        b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H16\r\n17\r\n18\r\n19\r\n20",
    );
    let frozen =
        super::read_terminal_snapshot(&runtime, ReadSource::Visible, ReadFormat::Text, None);
    assert_eq!(frozen.returned_lines, 5);
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);

    // A private listener satisfies the headless harness without starting a live server.
    let socket_dir = std::env::temp_dir().join(format!(
        "bus-fr-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir(&socket_dir).unwrap();
    let socket_path = socket_dir.join("client.sock");
    let listener = crate::platform::ipc::bind_local_listener(&socket_path).unwrap();
    let identity = crate::platform::ipc::socket_file_identity(&socket_path).unwrap();
    let (server_event_tx, server_event_rx) = tokio::sync::mpsc::channel(64);
    let headless_size = app.state.headless_size;
    let mut server = HeadlessServer {
        app,
        _api_tx: None,
        _api_server: None,
        client_listener: listener,
        client_socket_path: socket_path,
        client_socket_identity: identity,
        clients: HashMap::new(),
        next_client_id: 1,
        foreground_client_id: None,
        tab_geometry_controllers: HashMap::new(),
        client_shell_boot_id: "frozen-read-test".into(),
        sent_window_title: None,
        api_window_title: None,
        server_config_diagnostic: None,
        pending_alt_screen_reads: Vec::new(),
        deferred_alt_screen_reads: Vec::new(),
        next_activity_stamp: 1,
        headless_size,
        effective_size: headless_size,
        shutting_down: false,
        should_quit: Arc::new(AtomicBool::new(false)),
        server_event_rx,
        server_event_tx,
    };
    let request = |id: &str, source, lines| Request {
        id: id.into(),
        method: Method::PaneRead(PaneReadParams {
            pane_id: public_pane_id.clone(),
            source,
            lines,
            format: ReadFormat::Text,
            strip_ansi: true,
            intent: ReadIntent::Interactive,
        }),
    };
    let (respond_to, history_response) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check_inner(
        crate::server::api::ApiRequestMessage {
            request: request("history", ReadSource::Recent, Some(8)),
            respond_to,
        },
        true,
    );
    assert_eq!(server.pending_alt_screen_reads.len(), 1);
    assert!(history_response.try_recv().is_err());

    // An application's partial scroll redraw must not alter the frozen response facts.
    server
        .app
        .terminal_runtimes
        .get(&terminal_id)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[2J\x1b[H13\r\n14\r\n15");
    let (respond_to, visible_response) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check_inner(
        crate::server::api::ApiRequestMessage {
            request: request("visible", ReadSource::Visible, None),
            respond_to,
        },
        true,
    );
    let success: SuccessResponse =
        serde_json::from_str(&visible_response.try_recv().unwrap()).unwrap();
    let ResponseResult::PaneRead { read } = success.result else {
        panic!("expected frozen pane read");
    };
    drop(server);
    std::fs::remove_dir(socket_dir).unwrap();

    assert_eq!(read.text, frozen.text);
    assert_eq!(
        (
            read.returned_lines,
            read.revision,
            read.viewport_rows,
            read.viewport_columns
        ),
        (
            frozen.returned_lines,
            frozen.revision,
            frozen.viewport_rows,
            frozen.viewport_columns
        ),
        "frozen text must retain the range facts and revision of the same snapshot",
    );
}
