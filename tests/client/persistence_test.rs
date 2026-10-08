#[test]
fn cross_area_detach_and_reattach_preserves_state() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let server = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Local attach (client A).
    let mut client_a = UnixStream::connect(&client_socket).expect("client A should connect");
    client_shell_handshake(&mut client_a, CURRENT_PROTOCOL, 100, 30).expect("shell handshake");
    assert!(wait_for_frame(&mut client_a, Duration::from_secs(2)));

    // Use herdr: create a workspace and write output into its pane.
    let create = workspace_create(&api_socket, "cross-ssh-state");
    let workspace_id = create["result"]["workspace"]["workspace_id"]
        .as_str()
        .expect("workspace id")
        .to_string();
    let pane_id = create["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();

    pane_send_input(&api_socket, &pane_id, "echo LOCAL_BEFORE_DETACH");
    assert!(pane_read_recent_contains(
        &api_socket,
        &pane_id,
        "LOCAL_BEFORE_DETACH",
        Duration::from_secs(5)
    ));

    // Detach local client.
    send_client_detach(&mut client_a);
    drop(client_a);

    // Simulate activity while detached.
    pane_send_text(&api_socket, &pane_id, "echo DETACHED_UPDATE\n");
    assert!(pane_read_recent_contains(
        &api_socket,
        &pane_id,
        "DETACHED_UPDATE",
        Duration::from_secs(5)
    ));

    // Reattach from another terminal/session (client B).
    let mut client_b = UnixStream::connect(&client_socket).expect("client B should connect");
    client_shell_handshake(&mut client_b, CURRENT_PROTOCOL, 80, 24).expect("shell handshake");
    assert!(
        wait_for_frame(&mut client_b, Duration::from_secs(5)),
        "reattached client should receive frame"
    );

    let listed = workspace_list(&api_socket);
    assert_eq!(
        workspace_id,
        workspace_id_by_label(&listed, "cross-ssh-state"),
        "reattached session should see same workspace"
    );

    let readback = pane_read_recent(&api_socket, &pane_id);
    assert!(
        readback.contains("DETACHED_UPDATE"),
        "pane output should include detached-period output: {readback}"
    );

    cleanup_spawned_herdr(server, base);
}

#[test]
fn cross_area_agent_process_survives_detach_and_reattach() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let bin_dir = base.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_pi = bin_dir.join("pi");
    fs::write(&fake_pi, "#!/bin/sh\nprintf 'Working...\\n'\nsleep 30\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_pi).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_pi, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let path_override = format!("{}:{}", bin_dir.display(), inherited_path);

    let server = spawn_server_with_path(
        &config_home,
        &runtime_dir,
        &api_socket,
        Some(Path::new(&path_override)),
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut client_a = UnixStream::connect(&client_socket).expect("client A should connect");
    client_shell_handshake(&mut client_a, CURRENT_PROTOCOL, 100, 30).expect("shell handshake");
    assert!(wait_for_frame(&mut client_a, Duration::from_secs(2)));

    let created = workspace_create(&api_socket, "agent-persist");
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();

    // Ensure detected agent surface is populated by running fake `pi`.
    pane_send_text(&api_socket, &pane_id, "pi");
    pane_send_input(&api_socket, &pane_id, "");
    let detected_before_detach = {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut detected = false;
        while Instant::now() < deadline {
            let response = send_json_request(
                &api_socket,
                "pane_get",
                "pane.get",
                json!({ "pane_id": &pane_id }),
            );
            if response["result"]["pane"]["agent"].as_str() == Some("pi") {
                detected = true;
                break;
            }
            thread::sleep(Duration::from_millis(60));
        }
        detected
    };
    assert!(
        detected_before_detach,
        "expected fake pi process to be detected before detach"
    );

    // The fake process prints Pi's working marker, so screen detection drives the status;
    // confirming it takes a few seconds.
    assert!(
        wait_for_agent_status(&api_socket, &pane_id, "working", Duration::from_secs(10)),
        "pane agent status should become working before detach"
    );

    // Detach and ensure status persists through API while detached.
    send_client_detach(&mut client_a);
    drop(client_a);

    assert!(
        wait_for_agent_status(&api_socket, &pane_id, "working", Duration::from_secs(3)),
        "agent status should remain working while detached"
    );

    // Reattach and verify the persisted projection through the API.
    let mut client_b = UnixStream::connect(&client_socket).expect("client B should connect");
    client_shell_handshake(&mut client_b, CURRENT_PROTOCOL, 80, 24).expect("shell handshake");
    assert!(wait_for_frame(&mut client_b, Duration::from_secs(5)));

    let pane = send_json_request(
        &api_socket,
        "pane_get",
        "pane.get",
        json!({ "pane_id": &pane_id }),
    );
    assert_eq!(pane["result"]["pane"]["agent"], "pi", "{pane}");
    assert!(
        wait_for_agent_status(&api_socket, &pane_id, "working", Duration::from_secs(3)),
        "agent status should remain working after reattach"
    );

    cleanup_spawned_herdr(server, base);
}

#[test]
fn cross_area_server_kill_then_restart_and_reconnect() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let mut server = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Attach a real thin client process and prove it reached attached state
    // by observing an incoming frame on its PTY stream.
    let mut thin_client = spawn_client_process(&config_home, &runtime_dir, &api_socket);
    let mut thin_reader = thin_client
        ._master
        .as_ref()
        .expect("thin client master")
        .try_clone_reader()
        .expect("clone thin client reader");

    let attached_before_kill = {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut observed = false;
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline {
            match thin_reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let out = String::from_utf8_lossy(&buf[..n]);
                    if out.contains("\u{2500}")
                        || out.contains("$")
                        || out.contains("workspace")
                        || out.contains("pane")
                        || out.contains("terminal")
                    {
                        observed = true;
                        break;
                    }
                }
                Ok(_) => thread::sleep(Duration::from_millis(30)),
                Err(_) => thread::sleep(Duration::from_millis(30)),
            }
        }
        observed
    };
    assert!(
        attached_before_kill,
        "thin client should complete attach before server SIGKILL"
    );

    // Kill server abruptly and verify thin client exits with lost-connection messaging.
    let server_pid = server.child.process_id().expect("server pid should exist");
    unsafe {
        libc::kill(server_pid as libc::pid_t, libc::SIGKILL);
    }
    server.close_master();
    assert!(
        wait_for_child_exit(&mut server.child, Duration::from_secs(5)),
        "server should exit after SIGKILL"
    );
    drop(server);

    let mut crash_output = String::new();
    let thin_exited = {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut exited = false;
        let mut buf = [0u8; 1024];
        while Instant::now() < deadline {
            if thin_client.child.try_wait().ok().flatten().is_some() {
                exited = true;
                break;
            }
            if let Ok(n) = thin_reader.read(&mut buf) {
                if n > 0 {
                    crash_output.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        exited
    };
    assert!(thin_exited, "thin client should exit after server SIGKILL");

    let thin_status = thin_client
        .child
        .wait()
        .expect("wait for thin client exit status");
    assert!(
        !thin_status.success(),
        "thin client should exit non-zero after unexpected server crash"
    );

    // Drain trailing output and require the explicit user-visible lost-connection message.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match thin_reader.read(&mut buf) {
            Ok(n) if n > 0 => crash_output.push_str(&String::from_utf8_lossy(&buf[..n])),
            Ok(_) => break,
            Err(_) => break,
        }
        thread::sleep(Duration::from_millis(30));
    }

    let crash_output_lc = crash_output.to_lowercase();
    assert!(
        crash_output_lc.contains("lost connection to server"),
        "thin client output must include explicit lost-connection message after server kill; output: {crash_output:?}"
    );

    // Restart server and verify new client can connect (stale socket cleaned).
    let server2 = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut reconnect_client =
        UnixStream::connect(&client_socket).expect("new client should connect after restart");
    client_shell_handshake(&mut reconnect_client, CURRENT_PROTOCOL, 80, 24)
        .expect("shell handshake");
    assert!(
        wait_for_frame(&mut reconnect_client, Duration::from_secs(5)),
        "new client should receive frame after restart"
    );

    let ping = ping_socket(&api_socket);
    assert!(
        ping.contains("pong"),
        "restarted server should respond over API: {ping}"
    );

    cleanup_spawned_herdr(server2, base);
}
