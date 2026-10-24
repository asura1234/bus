fn wait_for_pane_tty_size(
    socket: &PathBuf,
    pane: &str,
    marker: &str,
    expected: (u16, u16),
) -> (u16, u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut sample = 0;
    loop {
        // A fresh marker prevents an earlier stty result from satisfying this sample.
        let size = read_pane_tty_size_after_marker(
            socket,
            pane,
            &format!("{marker}_{sample}"),
            deadline.saturating_duration_since(Instant::now()),
        );
        if size == expected {
            return size;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, last tty size {size:?}"
        );
        sample += 1;
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn pane_created_without_client_uses_configured_headless_size() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");

    let spawned = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        CUSTOM_HEADLESS_SIZE_CONFIG,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));

    let create = workspace_create(&api_socket, "headless-size");
    let pane_id = create["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();

    // API replies precede rendering; wait for the actual PTY resize.
    let size = wait_for_pane_tty_size(&api_socket, &pane_id, "HEADLESS_SIZE", (41, 132));

    assert_eq!(size, (41, 132));

    cleanup_spawned_bus(spawned, base);
}

#[test]
fn pane_created_after_detach_uses_configured_headless_size() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        CUSTOM_HEADLESS_SIZE_CONFIG,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("client should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 160, 50)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    support::wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(5))
        .expect("client shell bootstrap");

    let first = workspace_create(&api_socket, "attached-size");
    let first_pane_id = first["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("first root pane id")
        .to_string();
    let attached_size =
        wait_for_pane_tty_size(&api_socket, &first_pane_id, "ATTACHED_SIZE", (50, 160));
    assert_eq!(attached_size, (50, 160));

    send_detach(&mut stream).expect("send detach");
    assert!(
        wait_for_disconnect(&mut stream, Duration::from_secs(60)).expect("wait for detach"),
        "detached client connection should close"
    );
    drop(stream);

    let second = workspace_create(&api_socket, "headless-size");
    let second_pane_id = second["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("second root pane id")
        .to_string();
    let headless_size = wait_for_pane_tty_size(
        &api_socket,
        &second_pane_id,
        "HEADLESS_SIZE_AFTER_DETACH",
        (41, 132),
    );
    let preserved_size = read_pane_tty_size_after_marker(
        &api_socket,
        &first_pane_id,
        "PRESERVED_SIZE_AFTER_DETACH",
        Duration::from_secs(60),
    );

    assert_eq!(headless_size, (41, 132));
    assert_eq!(preserved_size, attached_size);

    cleanup_spawned_bus(spawned, base);
}

#[test]
fn detached_output_preserves_last_attached_pty_size() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("client should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 120, 40)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);
    drain_messages(&mut stream);

    let create = workspace_create(&api_socket, "detached-size");
    let pane_id = create["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();

    let before = read_pane_tty_size_after_marker(
        &api_socket,
        &pane_id,
        "SIZE_BEFORE_DETACH",
        Duration::from_secs(5),
    );

    send_detach(&mut stream).expect("send detach");
    drop(stream);

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should persist after detach"
    );

    let while_detached = read_pane_tty_size_after_marker(
        &api_socket,
        &pane_id,
        "SIZE_WHILE_DETACHED",
        Duration::from_secs(5),
    );

    assert_eq!(
        while_detached, before,
        "detached renders should not resize live pane PTYs to a fallback size"
    );

    cleanup_spawned_bus(spawned, base);
}
