#[test]
fn client_receives_pane_surface_after_pane_output() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(10))
        .expect("initial client shell bootstrap");

    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "create-output-workspace",
            "method": "workspace.create",
            "params": {"label": "output", "focus": true}
        })
        .to_string(),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id");
    assert!(wait_for_message_variant(
        &mut stream,
        Duration::from_secs(5),
        SERVER_MESSAGE_PANE_SURFACE,
    )
    .expect("wait for created workspace surface"));

    let sent = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "send-output",
            "method": "pane.send_text",
            "params": {"pane_id": pane_id, "text": "printf 'test-output\\n'\\n"}
        })
        .to_string(),
    );
    assert!(sent.get("error").is_none(), "{sent}");
    assert!(
        wait_for_message_variants(
            &mut stream,
            Duration::from_secs(5),
            &[
                SERVER_MESSAGE_PANE_SURFACE,
                SERVER_MESSAGE_PANE_SURFACE_PATCH,
            ],
        )
        .expect("wait for post-output pane surface"),
        "should receive a pane surface update after pane output"
    );

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn client_receives_notify_on_agent_state_change() {
    // Agent toasts are forwarded to connected clients as semantic
    // notifications when a detected agent's screen state changes.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    // A fake `pi` starts idle, shows Pi's working marker once the go file
    // appears, then clears the screen when the stop file appears and stays
    // alive so screen detection reports idle again.
    let bin_dir = base.join("bin");
    let go_file = base.join("pi-go");
    let stop_file = base.join("pi-stop");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_pi = bin_dir.join("pi");
    fs::write(
        &fake_pi,
        format!(
            "#!/bin/sh\nwhile [ ! -f '{go}' ]; do sleep 0.05; done\nprintf 'Working...\\n'\nwhile [ ! -f '{stop}' ]; do sleep 0.05; done\nprintf '\\033[2J\\033[Hdone\\n'\nsleep 30\n",
            go = go_file.display(),
            stop = stop_file.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_pi).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_pi, perms).unwrap();
    }
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let path_override = format!("{}:{}", bin_dir.display(), inherited_path);

    // Enable toasts in config so the server produces notifications.
    fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
    fs::write(
        config_home.join(app_dir_name()).join("config.toml"),
        "onboarding = false\n[ui.toast]\ndelivery = \"herdr\"\n",
    )
    .unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);

    // Spawn the server directly (not using spawn_server helper because it
    // overwrites the config file with a minimal one).
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", &api_socket);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env("PATH", &path_override);
    cmd.env_remove("HERDR_ENV");
    cmd.env_remove("BUS_DATA_DIR");
    cmd.env_remove("BUS_SESSION_ID");
    cmd.env_remove("HERDR_SESSION");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedHerdr {
        _master: Some(pair.master),
        child,
    };
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(5))
        .expect("client shell bootstrap");

    // Create a workspace via the API.
    let mut ws_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let request = r#"{"id":"1","method":"workspace.create","params":{}}"#;
    writeln!(ws_stream, "{}", request).unwrap();
    let mut reader = BufReader::new(ws_stream);
    let mut ws_response = String::new();
    reader.read_line(&mut ws_response).unwrap();

    // Extract the workspace ID and pane ID from the response.
    let ws_id = ws_response
        .split('"')
        .find(|s| s.starts_with("w_"))
        .unwrap_or("w_1")
        .to_string();

    // Get pane list to find a pane ID.
    let mut pane_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let pane_request =
        format!(r#"{{"id":"2","method":"pane.list","params":{{"workspace_id":"{ws_id}"}}}}"#);
    writeln!(pane_stream, "{}", pane_request).unwrap();
    let mut pane_reader = BufReader::new(pane_stream);
    let mut pane_response = String::new();
    pane_reader.read_line(&mut pane_response).unwrap();

    // Extract first pane ID (format: p_<ws>_<pane>).
    let pane_id = pane_response
        .split('"')
        .find(|s| s.starts_with("p_"))
        .unwrap_or("p_1_1")
        .to_string();

    // Start the fake agent. The first idle after detection never notifies.
    let started = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "3",
            "method": "pane.send_input",
            "params": { "pane_id": &pane_id, "text": "pi", "keys": ["Enter"] },
        })
        .to_string(),
    );
    assert_eq!(started["result"]["type"], "ok", "{started}");
    let pane_get =
        format!(r#"{{"id":"pane","method":"pane.get","params":{{"pane_id":"{pane_id}"}}}}"#);
    assert!(
        wait_until(Duration::from_secs(10), Duration::from_millis(50), || {
            let pane = send_json_request(&api_socket, &pane_get);
            pane["result"]["pane"]["agent"] == "pi"
                && pane["result"]["pane"]["agent_status"] == "idle"
        }),
        "fake pi should be detected as idle"
    );

    // Create and focus a second workspace so the agent's pane is in the background.
    let mut ws2_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let ws2_request = r#"{"id":"4","method":"workspace.create","params":{}}"#;
    writeln!(ws2_stream, "{}", ws2_request).unwrap();
    let mut ws2_reader = BufReader::new(ws2_stream);
    let mut ws2_response = String::new();
    ws2_reader.read_line(&mut ws2_response).unwrap();

    let ws2_id = ws2_response
        .split('"')
        .find(|s| s.starts_with("w_"))
        .unwrap_or("w_2")
        .to_string();
    let mut focus_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let focus_request = format!(
        r#"{{"id":"5","method":"workspace.focus","params":{{"workspace_id":"{ws2_id}"}}}}"#
    );
    writeln!(focus_stream, "{}", focus_request).unwrap();
    let mut focus_reader = BufReader::new(focus_stream);
    let mut focus_response = String::new();
    focus_reader.read_line(&mut focus_response).unwrap();

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should stay responsive after workspace focus"
    );

    // Working→Idle in a background workspace is a Done toast.
    fs::write(&go_file, "go").unwrap();
    assert!(
        wait_until(Duration::from_secs(10), Duration::from_millis(50), || {
            send_json_request(&api_socket, &pane_get)["result"]["pane"]["agent_status"] == "working"
        }),
        "fake pi should be detected as working"
    );
    fs::write(&stop_file, "stop").unwrap();

    // Read messages and look for the done semantic notification.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut found_done_notify = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match read_server_message(&mut stream) {
            Ok((variant, _payload)) => {
                if variant == SERVER_MESSAGE_SEMANTIC_NOTIFICATION {
                    found_done_notify = true;
                    break;
                }
                // Snapshot and pane-surface messages may arrive first.
            }
            Err(e) => {
                eprintln!("read error while looking for done notification: {e}");
                break;
            }
        }
    }

    assert!(
        found_done_notify,
        "client should receive a semantic notification when a background pane transitions Working→Idle"
    );

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn client_receives_notify_when_detected_agent_becomes_blocked() {
    // pane.report_agent 已删除，阻塞通知只能靠屏幕检测。amp 把
    // “waiting for approval” 判成 blocked，空屏是 idle，这样才能走出
    // idle→blocked 并发出 needs-attention 通知。
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let bin_dir = base.join("bin");
    let go_file = base.join("amp-go");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_amp = bin_dir.join("amp");
    fs::write(
        &fake_amp,
        format!(
            "#!/bin/sh\nwhile [ ! -f '{go}' ]; do sleep 0.05; done\nprintf 'waiting for approval\\n'\nsleep 30\n",
            go = go_file.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_amp).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_amp, perms).unwrap();
    }
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let path_override = format!("{}:{}", bin_dir.display(), inherited_path);

    fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
    fs::write(
        config_home.join(app_dir_name()).join("config.toml"),
        "onboarding = false\n[ui.toast]\ndelivery = \"herdr\"\n",
    )
    .unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", &api_socket);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env("PATH", &path_override);
    cmd.env_remove("HERDR_ENV");
    cmd.env_remove("BUS_DATA_DIR");
    cmd.env_remove("BUS_SESSION_ID");
    cmd.env_remove("HERDR_SESSION");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedHerdr {
        _master: Some(pair.master),
        child,
    };
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(5))
        .expect("client shell bootstrap");

    let created = send_json_request(
        &api_socket,
        r#"{"id":"1","method":"workspace.create","params":{}}"#,
    );
    let ws_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap_or("w_1")
        .to_string();
    let pane_id = first_pane_id_in_workspace(&api_socket, &ws_id);

    let started = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "3",
            "method": "pane.send_input",
            "params": { "pane_id": &pane_id, "text": "amp", "keys": ["Enter"] },
        })
        .to_string(),
    );
    assert_eq!(started["result"]["type"], "ok", "{started}");
    let pane_get =
        format!(r#"{{"id":"pane","method":"pane.get","params":{{"pane_id":"{pane_id}"}}}}"#);
    assert!(
        wait_until(Duration::from_secs(10), Duration::from_millis(50), || {
            let pane = send_json_request(&api_socket, &pane_get);
            pane["result"]["pane"]["agent"] == "amp"
                && pane["result"]["pane"]["agent_status"] == "idle"
        }),
        "fake amp should be detected as idle before the blocker appears"
    );

    support::drain_messages(&mut stream);
    fs::write(&go_file, "go").unwrap();

    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut found_blocked_notify = false;
    let mut saw_blocked = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !(found_blocked_notify && saw_blocked) {
        match read_server_message(&mut stream) {
            Ok((variant, payload)) => {
                if variant == SERVER_MESSAGE_SEMANTIC_NOTIFICATION
                    && payload
                        .windows(b"needs attention".len())
                        .any(|window| window == b"needs attention")
                {
                    found_blocked_notify = true;
                }
            }
            Err(error)
                if error.contains("timed out")
                    || error.contains("Resource temporarily unavailable") => {}
            Err(error) => {
                eprintln!("read error while looking for blocked notification: {error}");
                break;
            }
        }
        if !saw_blocked {
            saw_blocked = send_json_request(&api_socket, &pane_get)["result"]["pane"]
                ["agent_status"]
                == "blocked";
        }
    }

    assert!(saw_blocked, "fake amp should be detected as blocked");
    assert!(
        found_blocked_notify,
        "client should receive a needs-attention notification when a detected agent becomes blocked"
    );

    cleanup_spawned_herdr(spawned, base);
}
