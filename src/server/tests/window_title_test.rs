use super::*;

#[tokio::test]
async fn headless_api_reads_latest_title_without_spinner_event_flooding() {
    let event_hub = api::EventHub::default();
    let mut server = test_headless_server_with_event_hub(event_hub.clone());
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("one")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = server.app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .detected_agent = Some(crate::detect::Agent::Claude);
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"");
    runtime.test_process_pty_bytes(b"\x1b]0;\xe2\xa0\x8b task\x07");
    server
        .app
        .terminal_runtimes
        .insert(terminal_id.clone(), runtime);
    server.app.render_dirty.request_terminal_title(pane_id);

    let first = headless_pane_list(&mut server).pop().unwrap();
    assert_eq!(first.terminal_title.as_deref(), Some("⠋ task"));
    assert_eq!(first.terminal_title_stripped.as_deref(), Some("task"));
    assert_eq!(pane_updated_events(&event_hub), 1);
    server
        .app
        .terminal_runtimes
        .get(&terminal_id)
        .unwrap()
        .test_process_pty_bytes(b"\x1b]2;\xe2\xa0\x99 task\x1b\\");
    server.app.render_dirty.request_terminal_title(pane_id);
    let second = headless_pane_list(&mut server).pop().unwrap();
    assert_eq!(second.terminal_title.as_deref(), Some("⠙ task"));
    assert_eq!(second.terminal_title_stripped.as_deref(), Some("task"));
    assert_eq!(pane_updated_events(&event_hub), 1);
}

#[test]
fn window_title_waits_for_a_foreground_client_to_exist() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("herd")];
    server.app.state.active = Some(0);
    server.app.configure_window_title("{workspace}");

    // The server renders before the first client attaches. Nothing was
    // delivered, so nothing may be recorded as delivered either.
    server.sync_window_title();
    assert_eq!(server.sent_window_title, None);

    let (client_tx, control_rx, _render_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            1,
            client_tx,
        ),
    );
    server.promote_client_to_foreground(1);
    server.sync_window_title();

    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("herd".to_string()))
    );
    shutdown_test_runtimes(&mut server);
}

#[test]
fn an_attaching_client_gets_the_title_even_when_it_has_not_changed() {
    let (mut server, first_control_rx) = window_title_test_server();
    server.app.configure_window_title("{workspace}");
    server.sync_window_title();
    assert_eq!(
        next_window_title(&first_control_rx),
        Some(Some("herd".to_string()))
    );

    // Client-shell connection assigns the foreground client directly rather than
    // going through promote_client_to_foreground, so the cache must notice
    // the new client on its own.
    let (client_tx, second_control_rx, _render_rx) = test_client_writer();
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            2,
            client_tx,
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_window_title();

    assert_eq!(
        next_window_title(&second_control_rx),
        Some(Some("herd".to_string()))
    );
    shutdown_test_runtimes(&mut server);
}

#[test]
fn configured_window_title_reaches_the_foreground_client_once_per_change() {
    let (mut server, control_rx) = window_title_test_server();
    server.app.configure_window_title("{workspace}/{tab}");

    server.sync_window_title();
    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("herd/1".to_string()))
    );

    // An unchanged title must not re-emit an OSC on every render.
    server.sync_window_title();
    assert!(no_window_title(&control_rx));

    server.app.state.workspaces[0].tabs[0].custom_name = Some("build".into());
    server.sync_window_title();
    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("herd/build".to_string()))
    );

    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn focused_terminal_title_syncs_without_requesting_a_sidebar_render() {
    let (mut server, control_rx) = window_title_test_server();
    server.app.configure_window_title("{terminal_title}");
    server.app.state.ensure_test_terminals();
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = server.app.state.workspaces[0]
        .terminal_id(pane_id)
        .expect("terminal")
        .clone();
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"");
    runtime.test_process_pty_bytes("\x1b]0;⠋ building\x07".as_bytes());
    server
        .app
        .terminal_runtimes
        .insert(terminal_id.clone(), runtime);

    assert_eq!(
        server.sync_terminal_title_sources(&HashSet::from([pane_id])),
        (false, true)
    );
    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("building".to_string()))
    );

    server
        .app
        .terminal_runtimes
        .get(&terminal_id)
        .expect("runtime")
        .test_process_pty_bytes("\x1b]0;⠙ building\x07".as_bytes());
    assert_eq!(
        server.sync_terminal_title_sources(&HashSet::from([pane_id])),
        (false, true)
    );
    assert!(no_window_title(&control_rx));

    shutdown_test_runtimes(&mut server);
}

#[test]
fn empty_window_title_config_leaves_the_outer_title_alone() {
    let (mut server, control_rx) = window_title_test_server();
    server.app.configure_window_title("");

    server.sync_window_title();

    assert!(no_window_title(&control_rx));
    shutdown_test_runtimes(&mut server);
}

#[test]
fn api_window_title_wins_until_it_is_cleared() {
    let (mut server, control_rx) = window_title_test_server();
    server.app.configure_window_title("{workspace}");

    server.handle_client_window_title_api("set".into(), Some("herdr api".into()));
    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("herdr api".to_string()))
    );

    server.app.state.workspaces[0].custom_name = Some("ops".into());
    server.sync_window_title();
    assert!(no_window_title(&control_rx));

    // Clearing hands the title back to ui.window_title, not to "herdr".
    server.handle_client_window_title_api("clear".into(), None);
    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("ops".to_string()))
    );

    shutdown_test_runtimes(&mut server);
}

#[test]
fn clearing_the_api_title_falls_back_to_herdr_when_window_titles_are_disabled() {
    let (mut server, control_rx) = window_title_test_server();
    server.app.configure_window_title("");

    server.handle_client_window_title_api("set".into(), Some("herdr api".into()));
    assert_eq!(
        next_window_title(&control_rx),
        Some(Some("herdr api".to_string()))
    );

    server.handle_client_window_title_api("clear".into(), None);
    assert_eq!(next_window_title(&control_rx), Some(None));

    shutdown_test_runtimes(&mut server);
}

#[test]
fn a_newly_promoted_client_gets_the_window_title_again() {
    let (mut server, first_control_rx) = window_title_test_server();
    server.app.configure_window_title("{workspace}");
    server.sync_window_title();
    assert_eq!(
        next_window_title(&first_control_rx),
        Some(Some("herd".to_string()))
    );

    // A second terminal starts on whatever its shell or ssh left behind.
    let (client_tx, second_control_rx, _render_rx) = test_client_writer();
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            2,
            client_tx,
        ),
    );
    server.promote_client_to_foreground(2);
    server.sync_window_title();

    assert_eq!(
        next_window_title(&second_control_rx),
        Some(Some("herd".to_string()))
    );
    shutdown_test_runtimes(&mut server);
}
