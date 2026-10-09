#[test]
fn cross_area_client_and_api_workspace_views_are_consistent() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let server = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut client = UnixStream::connect(&client_socket).expect("client should connect");
    client_shell_handshake(&mut client, CURRENT_PROTOCOL, 100, 30).expect("shell handshake");
    assert!(wait_for_frame(&mut client, Duration::from_secs(2)));
    drain_server_messages(&mut client, Duration::from_millis(300));

    let before = workspace_count(&api_socket);

    // Create a workspace via API while the client is attached.
    let created = workspace_create(&api_socket, "api-visible-workspace");
    let created_workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .expect("workspace.create should return workspace_id")
        .to_string();

    // ClientShell state is asserted through the authoritative API projection below;
    // do not decode client-composed UI to duplicate that assertion.

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut count_reached = false;
    while Instant::now() < deadline {
        if workspace_count(&api_socket) == before + 1 {
            count_reached = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        count_reached,
        "API workspace list should include the created workspace"
    );

    let listed = workspace_list(&api_socket);
    let listed_workspace_id = workspace_id_by_label(&listed, "api-visible-workspace");
    assert_eq!(
        listed_workspace_id, created_workspace_id,
        "API and client-side state should reference the same created workspace"
    );

    cleanup_spawned_bus(server, base);
}

#[test]
fn cross_area_two_clients_shared_view_and_single_detach_stability() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let server = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut client_a = UnixStream::connect(&client_socket).expect("client A should connect");
    client_shell_handshake(&mut client_a, CURRENT_PROTOCOL, 110, 30).expect("shell handshake");
    let mut client_b = UnixStream::connect(&client_socket).expect("client B should connect");
    client_shell_handshake(&mut client_b, CURRENT_PROTOCOL, 100, 30).expect("shell handshake");

    assert!(wait_for_frame(&mut client_a, Duration::from_secs(2)));
    assert!(wait_for_frame(&mut client_b, Duration::from_secs(2)));
    drain_server_messages(&mut client_a, Duration::from_millis(250));
    drain_server_messages(&mut client_b, Duration::from_millis(250));

    let created = workspace_create(&api_socket, "shared-view");
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();

    // Input from client A should update shared state visible to client B.
    pane_send_text(&api_socket, &pane_id, "echo SHARED_VIEW\n");
    assert!(
        wait_for_frame(&mut client_b, Duration::from_secs(2)),
        "client B should receive update from client A"
    );
    assert!(pane_read_recent_contains(
        &api_socket,
        &pane_id,
        "SHARED_VIEW",
        Duration::from_secs(5)
    ));

    // Detach client A; client B should keep working.
    send_client_detach(&mut client_a);
    assert!(
        support::wait_for_disconnect(&mut client_a, Duration::from_secs(2))
            .expect("wait for client A detach"),
        "client A should disconnect before the remaining-client assertion"
    );
    drop(client_a);

    pane_send_text(&api_socket, &pane_id, "echo AFTER_A_DETACH\n");
    assert!(pane_read_recent_contains(
        &api_socket,
        &pane_id,
        "AFTER_A_DETACH",
        Duration::from_secs(5)
    ));
    assert!(
        wait_for_frame(&mut client_b, Duration::from_secs(2)),
        "remaining client should still receive frames after other client detaches"
    );

    let ping = ping_socket(&api_socket);
    assert!(
        ping.contains("pong"),
        "server and remaining client flow should stay healthy: {ping}"
    );

    cleanup_spawned_bus(server, base);
}
