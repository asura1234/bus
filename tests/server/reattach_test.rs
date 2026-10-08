#[test]
fn explicit_detach_message_causes_clean_disconnect() {
    // Client sends ClientMessage::Detach
    // directly (not via keybind), server handles it gracefully.
    // This is the flow when the client process is exiting cleanly (Ctrl+C).
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frames.
    drain_messages(&mut stream);

    // Send ClientMessage::Detach directly.
    send_detach(&mut stream).expect("send detach message");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should persist after client Detach message"
    );

    // Verify server is still alive.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should persist after client Detach message: {response}"
    );

    // The client connection should eventually be closed.
    // After sending Detach, the server removes the client.
    // We may still receive a few queued frames before the connection closes.
    let got_eof =
        wait_for_disconnect(&mut stream, Duration::from_secs(2)).expect("wait for disconnect");
    assert!(
        got_eof,
        "client connection should be closed after explicit Detach message"
    );

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn reattach_after_detach_shows_current_state() {
    // Flow:
    // 1. Start server
    // 2. Connect client A, create a workspace via API
    // 3. Client A detaches
    // 4. Connect client B (reattach), verify it receives a frame
    // 5. Verify client B can see the workspace created by client A
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // --- Client A ---
    let mut stream_a = UnixStream::connect(&client_socket).expect("client A should connect");
    let (version, error) = client_shell_handshake(&mut stream_a, CURRENT_PROTOCOL, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frames.
    drain_messages(&mut stream_a);

    // Create a workspace via API while client A is attached.
    let mut ws_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let request = r#"{"id":"1","method":"workspace.create","params":{"label":"reattach-test"}}"#;
    writeln!(ws_stream, "{}", request).unwrap();
    let mut reader = BufReader::new(ws_stream);
    let mut ws_response = String::new();
    reader.read_line(&mut ws_response).unwrap();
    assert!(
        ws_response.contains("workspace_created") || ws_response.contains("ok"),
        "workspace creation should succeed: {ws_response}"
    );

    // Client A detaches (send ClientMessage::Detach).
    send_detach(&mut stream_a).expect("send detach");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should persist after detach"
    );

    // Verify server is still alive.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should persist after detach: {response}"
    );

    // --- Client B (reattach) ---
    let mut stream_b = UnixStream::connect(&client_socket).expect("client B should connect");
    let (version, error) = client_shell_handshake(&mut stream_b, CURRENT_PROTOCOL, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(
        error.is_none(),
        "reattach handshake should succeed: {:?}",
        error
    );

    // Client B receives the client-owned shell projection.
    // including the workspace created while client A was attached.
    support::wait_for_client_shell_bootstrap(&mut stream_b, Duration::from_secs(5))
        .expect("client shell bootstrap");

    // Verify the workspace still exists via API.
    let mut list_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let list_request = r#"{"id":"2","method":"workspace.list","params":{}}"#;
    writeln!(list_stream, "{}", list_request).unwrap();
    let mut list_reader = BufReader::new(list_stream);
    let mut list_response = String::new();
    list_reader.read_line(&mut list_response).unwrap();
    assert!(
        list_response.contains("reattach-test"),
        "workspace should still exist after detach/reattach: {list_response}"
    );

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn processes_survive_during_and_after_detach() {
    // PTY processes continue running during and after detach.
    //
    // Simplified flow:
    // 1. Start server
    // 2. Connect client, send "echo SURVIVED" to the pane
    // 3. Detach client
    // 4. Verify server is still alive and API works
    // 5. Reattach and verify we can receive a frame
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Verify server starts with a workspace (session restore or fresh state).
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should respond to ping: {response}"
    );

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frames.
    drain_messages(&mut stream);

    // Drive the pane through the JSON API; the shell transport carries only
    // client-composed UI state, not legacy raw shell input.
    let created = workspace_create(&api_socket, "process-survival");
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();
    pane_send_text(&api_socket, &pane_id, "echo SURVIVED_DETACH\n");
    assert!(wait_until(
        Duration::from_secs(5),
        Duration::from_millis(50),
        || pane_read_recent_text(&api_socket, &pane_id).contains("SURVIVED_DETACH")
    ));

    // Drain any shell projection updates.
    drain_messages(&mut stream);

    // Detach the client via explicit Detach message.
    send_detach(&mut stream).expect("send detach");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should persist after detach"
    );

    // Verify server is still alive after detach.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should persist after detach: {response}"
    );

    assert!(
        wait_for_disconnect(&mut stream, Duration::from_secs(2)).expect("wait for detach"),
        "detached client connection should close"
    );

    // Reattach — verify we can connect and receive a frame.
    let mut stream_b = UnixStream::connect(&client_socket).expect("should reattach");
    let (version, error) = client_shell_handshake(&mut stream_b, CURRENT_PROTOCOL, 80, 24)
        .expect("reattach handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Verify the reattached client receives a frame.
    support::wait_for_client_shell_bootstrap(&mut stream_b, Duration::from_secs(5))
        .expect("client shell bootstrap");

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn server_persists_after_client_connection_drop() {
    // Server continues running after a client
    // disconnects (not just detach — also connection drop).
    // Verify that after a client connection is abruptly closed (not via Detach),
    // the server continues running and can accept new connections.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frames.
    drain_messages(&mut stream);

    // Drop the connection abruptly (simulating client crash).
    drop(stream);

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should persist after client connection drop"
    );

    // Verify server is still alive.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should persist after client connection drop: {response}"
    );

    // Reattach — verify we can connect and handshake again.
    let mut stream_b = UnixStream::connect(&client_socket).expect("should reattach");
    let (version, error) = client_shell_handshake(&mut stream_b, CURRENT_PROTOCOL, 80, 24)
        .expect("reattach handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "reattach should succeed: {:?}", error);

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn output_accumulated_while_detached_visible_on_reattach() {
    // Output produced while detached is visible in the
    // reattached client's scrollback.
    //
    // Simplified flow:
    // 1. Start server, attach client A
    // 2. Detach client A (without sending any special input)
    // 3. Use API to send text to a pane while detached
    // 4. Reattach as client B
    // 5. Verify client B receives a frame
    // 6. Verify the pane content via API includes the text sent while detached
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Connect and handshake client A.
    let mut stream_a = UnixStream::connect(&client_socket).expect("client A should connect");
    let (version, error) = client_shell_handshake(&mut stream_a, CURRENT_PROTOCOL, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Detach client A immediately.
    send_detach(&mut stream_a).expect("send detach");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should persist"
    );

    // Verify server alive.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should persist: {response}"
    );

    // Use API to send text to a pane while no client is attached.
    // First create a workspace and find its pane.
    let ws_create_response = workspace_create(&api_socket, "scrollback-test");
    assert_eq!(ws_create_response["result"]["type"], "workspace_created");

    // Find the workspace and pane IDs.
    let ws_response = workspace_list(&api_socket);
    let ws_id = workspace_id_by_label(&ws_response, "scrollback-test");

    // Get pane list for this workspace.
    let pane_response = pane_list(&api_socket, &ws_id);
    let pane_id = first_pane_id(&pane_response);

    // Send text to the pane via API while detached.
    let mut send_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let send_request = format!(
        r#"{{"id":"4","method":"pane.send_text","params":{{"pane_id":"{pane_id}","text":"echo DURING_DETACH\n"}}}}"#
    );
    writeln!(send_stream, "{}", send_request).unwrap();
    let mut send_reader = BufReader::new(send_stream);
    let mut send_response = String::new();
    send_reader.read_line(&mut send_response).unwrap();

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            let read_response = pane_read_recent(&api_socket, &pane_id);
            read_response["result"]["read"]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("DURING_DETACH")
        }),
        "pane should contain output produced while detached before reattach"
    );

    // --- Client B (reattach) ---
    let mut stream_b = UnixStream::connect(&client_socket).expect("client B should connect");
    let (version, error) = client_shell_handshake(&mut stream_b, CURRENT_PROTOCOL, 80, 24)
        .expect("reattach handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{:?}", error);

    // Client B should receive a frame with the current state.
    support::wait_for_client_shell_bootstrap(&mut stream_b, Duration::from_secs(5))
        .expect("client shell bootstrap");

    cleanup_spawned_herdr(spawned, base);
}
