const NATIVE_FIXTURE_WAIT: Duration = Duration::from_secs(60);

fn wait_for_fake_provider_ready(socket: &PathBuf, pane: &str, marker: &str) {
    let request = serde_json::json!({
        "id": "fake-ready",
        "method": "pane.read",
        "params": {"pane_id": pane, "source": "recent", "lines": 200}
    })
    .to_string();
    assert!(
        wait_until(NATIVE_FIXTURE_WAIT, Duration::from_millis(50), || {
            send_json_request(socket, &request)["result"]["read"]["text"]
                .as_str()
                .is_some_and(|text| text.contains(marker))
        }),
        "fake provider did not reach readiness barrier {marker}"
    );
}

fn wait_for_semantic_notification(stream: &mut UnixStream, pane: &str, event: &str) -> bool {
    let deadline = Instant::now() + NATIVE_FIXTURE_WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        stream.set_read_timeout(Some(remaining)).unwrap();
        // A timeout can consume part of a frame. Do not discard that error
        // and treat the remaining payload as a new length prefix.
        let (variant, payload) = match read_server_message(stream) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("read error while awaiting {event} for {pane}: {error}");
                return false;
            }
        };
        if variant == SERVER_MESSAGE_SEMANTIC_NOTIFICATION
            && payload
                .windows(event.len())
                .any(|window| window == event.as_bytes())
            && payload
                .windows(pane.len())
                .any(|window| window == pane.as_bytes())
        {
            return true;
        }
    }
}

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

    cleanup_spawned_bus(spawned, base);
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
    let exit_file = base.join("pi-exit");
    fs::create_dir_all(&bin_dir).unwrap();
    support::write_fake_agent(
        &bin_dir,
        "pi",
        &format!(
            "printf 'fake-pi-ready\\n'\nwhile [ ! -f '{go}' ]; do sleep 0.05; done\nprintf 'Working...\\n'\nwhile [ ! -f '{stop}' ]; do sleep 0.05; done\nprintf '\\033[2J\\033[Hdone\\n'\nwhile [ ! -f '{exit}' ]; do sleep 0.05; done\n",
            go = go_file.display(),
            stop = stop_file.display(),
            exit = exit_file.display()
        ),
    );
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
    cmd.env("HOME", runtime_dir.join("home"));
    cmd.env("USERPROFILE", runtime_dir.join("home"));
    cmd.env("HERDR_SOCKET_PATH", &api_socket);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env("PATH", &path_override);
    cmd.env_remove("HERDR_ENV");
    cmd.env_remove("BUS_DATA_DIR");
    cmd.env_remove("BUS_SESSION_ID");
    cmd.env_remove("HERDR_SESSION");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_bus_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedBus {
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

    // Use the canonical public pane ID: accepted legacy aliases are not
    // the identity carried in semantic notifications.
    let created = send_json_request(
        &api_socket,
        r#"{"id":"1","method":"workspace.create","params":{"focus":true}}"#,
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("created workspace root pane ID")
        .to_owned();

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
    wait_for_fake_provider_ready(&api_socket, &pane_id, "fake-pi-ready");
    let pane_get =
        format!(r#"{{"id":"pane","method":"pane.get","params":{{"pane_id":"{pane_id}"}}}}"#);
    assert!(
        wait_until(NATIVE_FIXTURE_WAIT, Duration::from_millis(50), || {
            let pane = send_json_request(&api_socket, &pane_get);
            pane["result"]["pane"]["agent"] == "pi"
                && pane["result"]["pane"]["agent_status"] == "idle"
        }),
        "fake pi should be detected as idle"
    );

    // Create and focus a second workspace so the agent's pane is in the background.
    let background = send_json_request(
        &api_socket,
        r#"{"id":"4","method":"workspace.create","params":{}}"#,
    );
    let background_id = background["result"]["workspace"]["workspace_id"]
        .as_str()
        .expect("background workspace ID");
    let focused = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "5",
            "method": "workspace.focus",
            "params": {"workspace_id": background_id},
        })
        .to_string(),
    );
    assert!(focused.get("error").is_none(), "{focused}");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should stay responsive after workspace focus"
    );

    // Working→Idle in a background workspace is a Done toast.
    fs::write(&go_file, "go").unwrap();
    assert!(
        wait_until(NATIVE_FIXTURE_WAIT, Duration::from_millis(50), || {
            send_json_request(&api_socket, &pane_get)["result"]["pane"]["agent_status"] == "working"
        }),
        "fake pi should be detected as working"
    );
    fs::write(&stop_file, "stop").unwrap();

    assert!(
        wait_for_semantic_notification(&mut stream, &pane_id, "finished"),
        "client should receive a semantic notification when a background pane transitions Working→Idle"
    );

    fs::write(&exit_file, "exit").unwrap();
    cleanup_spawned_bus(spawned, base);
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
    let stop_file = base.join("amp-stop");
    fs::create_dir_all(&bin_dir).unwrap();
    support::write_fake_agent(
        &bin_dir,
        "amp",
        &format!(
            "printf 'fake-amp-ready\\n'\nwhile [ ! -f '{go}' ]; do sleep 0.05; done\nprintf 'waiting for approval\\n'\nwhile [ ! -f '{stop}' ]; do sleep 0.05; done\n",
            go = go_file.display(),
            stop = stop_file.display()
        ),
    );
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
    cmd.env("HOME", runtime_dir.join("home"));
    cmd.env("USERPROFILE", runtime_dir.join("home"));
    cmd.env("HERDR_SOCKET_PATH", &api_socket);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env("PATH", &path_override);
    cmd.env_remove("HERDR_ENV");
    cmd.env_remove("BUS_DATA_DIR");
    cmd.env_remove("BUS_SESSION_ID");
    cmd.env_remove("HERDR_SESSION");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_bus_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedBus {
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
    wait_for_fake_provider_ready(&api_socket, &pane_id, "fake-amp-ready");
    let pane_get =
        format!(r#"{{"id":"pane","method":"pane.get","params":{{"pane_id":"{pane_id}"}}}}"#);
    assert!(
        wait_until(NATIVE_FIXTURE_WAIT, Duration::from_millis(50), || {
            let pane = send_json_request(&api_socket, &pane_get);
            pane["result"]["pane"]["agent"] == "amp"
                && pane["result"]["pane"]["agent_status"] == "idle"
        }),
        "fake amp should be detected as idle before the blocker appears"
    );

    fs::write(&go_file, "go").unwrap();
    assert!(
        wait_until(NATIVE_FIXTURE_WAIT, Duration::from_millis(50), || {
            send_json_request(&api_socket, &pane_get)["result"]["pane"]["agent_status"] == "blocked"
        }),
        "fake amp should be detected as blocked"
    );
    assert!(
        wait_for_semantic_notification(&mut stream, &pane_id, "needs attention"),
        "client should receive a needs-attention notification when a detected agent becomes blocked"
    );

    fs::write(&stop_file, "stop").unwrap();
    cleanup_spawned_bus(spawned, base);
}
