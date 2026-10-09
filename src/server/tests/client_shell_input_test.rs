use super::*;

#[tokio::test]
async fn client_shell_input_targets_runtime_without_server_shell_classification() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"\x1b[?1000h\x1b[?1006h");
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );

    assert!(
        server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 11,
            pane_id,
            events: vec![
                crate::protocol::wire::ClientPaneInputEvent::Key {
                    code: crate::protocol::wire::ClientKeyCode::Char('c'),
                    modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                    kind: crate::protocol::wire::ClientKeyKind::Press,
                    repeat_count: 1,
                    shifted_codepoint: None,
                    generated_text: None,
                    tracks_release: true,
                    physical_key_id: None,
                    windows_record: None,
                },
                crate::protocol::wire::ClientPaneInputEvent::Key {
                    code: crate::protocol::wire::ClientKeyCode::Char('c'),
                    modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                    kind: crate::protocol::wire::ClientKeyKind::Release,
                    repeat_count: 1,
                    shifted_codepoint: None,
                    generated_text: None,
                    tracks_release: true,
                    physical_key_id: None,
                    windows_record: None,
                },
                crate::protocol::wire::ClientPaneInputEvent::Key {
                    code: crate::protocol::wire::ClientKeyCode::Char('x'),
                    modifiers: crossterm::event::KeyModifiers::ALT.bits(),
                    kind: crate::protocol::wire::ClientKeyKind::Press,
                    repeat_count: 1,
                    shifted_codepoint: None,
                    generated_text: None,
                    tracks_release: true,
                    physical_key_id: None,
                    windows_record: None,
                },
                crate::protocol::wire::ClientPaneInputEvent::Mouse {
                    kind: crate::protocol::wire::ClientMouseKind::Down(
                        crate::protocol::wire::ClientMouseButton::Left,
                    ),
                    position: crate::protocol::wire::ClientMousePosition::Cell {
                        column: 2,
                        row: 1
                    },
                    geometry: None,
                    modifiers: 0,
                    lines: 3,
                },
            ],
        })
    );
    assert_eq!(
        input_rx.try_recv().expect("targeted pane interrupt"),
        Bytes::from_static(&[0x03])
    );
    assert_eq!(
        input_rx.try_recv().expect("targeted pane alt key"),
        Bytes::from_static(b"\x1bx")
    );
    assert_eq!(
        input_rx.try_recv().expect("targeted pane mouse click"),
        Bytes::from_static(b"\x1b[<0;3;2M")
    );
    assert_eq!(server.foreground_client_id, Some(11));
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    assert!(server.paste_client_clipboard_image_path(
        11,
        crate::protocol::wire::ClientClipboardImageTarget::Pane(pane_id.clone()),
        "/tmp/client-image.png".into(),
    ));
    assert_eq!(
        input_rx.try_recv().expect("targeted clipboard image path"),
        Bytes::from_static(b"/tmp/client-image.png")
    );
    assert!(!server.paste_client_clipboard_image_path(
        11,
        crate::protocol::wire::ClientClipboardImageTarget::Pane("missing:p1".into()),
        "/tmp/wrong-target.png".into(),
    ));
    assert!(input_rx.try_recv().is_err());

    let (workspace_index, runtime_pane_id) = server
        .app
        .parse_pane_id(&pane_id)
        .expect("runtime pane target");
    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(
            &server.app.terminal_runtimes,
            workspace_index,
            runtime_pane_id,
        )
        .expect("focused runtime");
    assert_eq!(runtime.current_size(), (24, 79));
    assert!(input_rx.try_recv().is_err(), "legacy release emitted bytes");
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_hidden_pane_rejects_presses_but_accepts_releases() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("hidden-input");
    let hidden_tab = workspace.test_add_tab(Some("hidden"));
    let hidden_pane = workspace.tabs[hidden_tab].root_pane;
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[>3u",
            4,
        );
    workspace.insert_test_runtime(hidden_pane, runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    let pane_id = server.app.public_pane_id(0, hidden_pane).unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );
    let key = |kind| crate::protocol::wire::ClientPaneInputEvent::Key {
        code: crate::protocol::wire::ClientKeyCode::Char('x'),
        modifiers: 0,
        kind,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: Some(0x2d),
        windows_record: None,
    };

    assert!(
        !server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 11,
            pane_id: pane_id.clone(),
            events: vec![key(crate::protocol::wire::ClientKeyKind::Press)],
        })
    );
    assert!(input_rx.try_recv().is_err());
    assert!(
        !server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 11,
            pane_id,
            events: vec![key(crate::protocol::wire::ClientKeyKind::Release)],
        })
    );
    assert!(!input_rx.recv().await.expect("encoded release").is_empty());
    assert_eq!(server.foreground_client_id, None);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_text_input_renders_only_when_resetting_scrollback() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("scrolled-input");
    let pane_id = workspace.tabs[0].root_pane;
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            2,
            10_000,
            b"one\r\ntwo\r\nthree\r\n",
            4,
        );
    runtime.scroll_up(1);
    assert!(runtime
        .scroll_metrics()
        .is_some_and(|metrics| metrics.offset_from_bottom > 0));
    workspace.insert_test_runtime(pane_id, runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );
    server.foreground_client_id = Some(11);

    let render_impact = server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id: public_pane_id.clone(),
        events: vec![crate::protocol::wire::ClientPaneInputEvent::TextCommit(
            "x".to_owned(),
        )],
    });

    assert!(render_impact);
    assert_eq!(
        input_rx.try_recv().expect("text must reach the PTY"),
        Bytes::from_static(b"x")
    );
    assert_eq!(
        server
            .app
            .state
            .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
            .and_then(|runtime| runtime.scroll_metrics())
            .map(|metrics| metrics.offset_from_bottom),
        Some(0)
    );

    let render_impact = server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id: public_pane_id,
        events: vec![crate::protocol::wire::ClientPaneInputEvent::TextCommit(
            "y".to_owned(),
        )],
    });
    assert!(!render_impact);
    assert_eq!(
        input_rx.try_recv().expect("second text must reach the PTY"),
        Bytes::from_static(b"y")
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_mouse_motion_delivers_without_render_when_foreground() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"\x1b[?1003h\x1b[?1006h");
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );
    server.foreground_client_id = Some(11);
    assert!(server.claim_unowned_shell_tab_geometry(11, false));

    let render_impact = server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id,
        events: vec![crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position: crate::protocol::wire::ClientMousePosition::Cell { column: 2, row: 1 },
            geometry: None,
            modifiers: 0,
            lines: 0,
        }],
    });

    assert!(!render_impact);
    assert!(
        input_rx.try_recv().is_ok(),
        "motion must still reach the PTY"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_mouse_motion_promotes_and_requests_render() {
    let mut server = test_headless_server();
    let mut input_rx = install_focused_test_runtime(&mut server, b"\x1b[?1003h\x1b[?1006h");
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );

    let render_impact = server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id,
        events: vec![crate::protocol::wire::ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position: crate::protocol::wire::ClientMousePosition::Cell { column: 2, row: 1 },
            geometry: None,
            modifiers: 0,
            lines: 0,
        }],
    });

    assert!(render_impact);
    assert_eq!(server.foreground_client_id, Some(11));
    assert!(
        input_rx.try_recv().is_ok(),
        "motion must still reach the PTY"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_failed_release_retains_disconnect_cleanup() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("failed-release");
    let runtime_pane_id = workspace.tabs[0].root_pane;
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[>3u",
            1,
        );
    workspace.insert_test_runtime(runtime_pane_id, runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    let pane_id = server.app.public_pane_id(0, runtime_pane_id).unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );
    let key = |kind| crate::protocol::wire::ClientPaneInputEvent::Key {
        code: crate::protocol::wire::ClientKeyCode::Char('x'),
        modifiers: 0,
        kind,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: Some(0x2d),
        windows_record: None,
    };
    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id: pane_id.clone(),
        events: vec![key(crate::protocol::wire::ClientKeyKind::Press)],
    });
    assert_eq!(
        input_rx.try_recv().expect("press must reach the live PTY"),
        Bytes::from_static(b"x")
    );

    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id: pane_id.clone(),
        events: vec![crate::protocol::wire::ClientPaneInputEvent::TextCommit(
            "pending".into(),
        )],
    });
    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id,
        events: vec![key(crate::protocol::wire::ClientKeyKind::Release)],
    });
    assert_eq!(
        input_rx
            .try_recv()
            .expect("queued input must survive saturation"),
        Bytes::from_static(b"pending")
    );
    assert!(input_rx.try_recv().is_err(), "release was not queued");

    server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 11 });
    assert_eq!(
        input_rx
            .try_recv()
            .expect("disconnect must release the key after the live PTY queue recovers"),
        Bytes::from_static(b"\x1b[120;1:3u")
    );
    assert!(input_rx.try_recv().is_err());
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn client_shell_disconnect_releases_only_accepted_batch_presses() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("partial-input-batch");
    let runtime_pane_id = workspace.tabs[0].root_pane;
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[>3u",
            2,
        );
    workspace.insert_test_runtime(runtime_pane_id, runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    let pane_id = server.app.public_pane_id(0, runtime_pane_id).unwrap();
    server.clients.insert(
        11,
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        ),
    );
    let key = |character, physical_key_id| crate::protocol::wire::ClientPaneInputEvent::Key {
        code: crate::protocol::wire::ClientKeyCode::Char(character),
        modifiers: 0,
        kind: crate::protocol::wire::ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: Some(physical_key_id),
        windows_record: None,
    };

    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id: pane_id.clone(),
        events: vec![crate::protocol::wire::ClientPaneInputEvent::TextCommit(
            "pending".into(),
        )],
    });
    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 11,
        pane_id,
        events: vec![key('x', 0x2d), key('y', 0x15)],
    });
    assert_eq!(input_rx.try_recv().unwrap(), Bytes::from_static(b"pending"));
    assert_eq!(input_rx.try_recv().unwrap(), Bytes::from_static(b"x"));
    assert!(
        input_rx.try_recv().is_err(),
        "the second press was not queued"
    );

    server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 11 });
    let mut releases = Vec::new();
    while let Ok(bytes) = input_rx.try_recv() {
        releases.push(bytes);
    }
    assert_eq!(
        releases,
        vec![Bytes::from_static(b"\x1b[120;1:3u")],
        "disconnect must not release a key whose press was rejected by the full live PTY queue"
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[test]
fn clipboard_image_staging_rejects_symlink_directory() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    const CHILD_ROOT: &str = "BUS_CLIPBOARD_STAGING_SYMLINK_TEST_ROOT";
    const TEST_NAME: &str =
        "server::tests::client_shell_input_tests::clipboard_image_staging_rejects_symlink_directory";

    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        let victim = root.join("victim");
        let before = std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777;
        let unrelated_file = victim.join("unrelated-document.txt");

        let staged = crate::server::clients::clipboard_images::stage(11, "png", b"image");

        let after = std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            (after, unrelated_file.exists(), staged.is_err()),
            (before, true, true),
            "clipboard staging must reject a preexisting staging-directory symlink without chmod or deletion in its target"
        );
        return;
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "bus-clipboard-staging-symlink-test-{}-{unique}",
        std::process::id()
    ));
    let shared_temp = root.join("shared-temp");
    let victim = root.join("victim");
    std::fs::create_dir_all(&shared_temp).unwrap();
    std::fs::create_dir(&victim).unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let unrelated_file = std::fs::File::create(victim.join("unrelated-document.txt")).unwrap();
    unrelated_file
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
        )
        .unwrap();
    let user_id = unsafe { libc::geteuid() };
    symlink(
        &victim,
        shared_temp.join(format!("bus-clipboard-images-{user_id}")),
    )
    .unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .env("TMPDIR", &shared_temp)
        .env(CHILD_ROOT, &root)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        output.status.success(),
        "isolated staging probe failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
