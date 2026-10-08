use super::*;

#[cfg(not(target_os = "macos"))]
#[test]
fn agent_start_targets_existing_pane_over_socket() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake_pi = bin.join("pi");
    fs::write(&fake_pi, "#!/bin/sh\nHERDR_AGENT=pi exec /bin/sleep 20\n").unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let child = spawn_herdr_with_path(&config_home, &runtime_dir, &socket_path, &bin);
    wait_for_socket(&socket_path, Duration::from_secs(5));
    let workspace = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "agent_workspace",
            "method": "workspace.create",
            "params": { "cwd": base.display().to_string(), "focus": false }
        })
        .to_string(),
    );
    let pane_id = workspace["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let terminal_id = workspace["result"]["root_pane"]["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();

    let started = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "agent_start",
            "method": "agent.start",
            "params": {
                "name": "main",
                "kind": "pi",
                "pane_id": pane_id,
                "args": ["--no-session"],
                "timeout_ms": 8_000
            }
        })
        .to_string(),
    );
    assert_eq!(started["result"]["type"], "agent_started");
    assert_eq!(started["result"]["agent"]["name"], "main");
    assert_eq!(started["result"]["agent"]["pane_id"], pane_id);
    assert_eq!(started["result"]["agent"]["terminal_id"], terminal_id);
    assert_eq!(
        started["result"]["argv"],
        serde_json::json!(["pi", "--no-session"])
    );

    let duplicate = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "agent_start_duplicate",
            "method": "agent.start",
            "params": {
                "name": "main",
                "kind": "pi",
                "pane_id": pane_id
            }
        })
        .to_string(),
    );
    assert_eq!(duplicate["error"]["code"], "agent_name_taken");
    assert!(duplicate["error"]["message"]
        .as_str()
        .unwrap()
        .contains(&terminal_id));

    cleanup_spawned_herdr(child, base);
}

#[test]
fn agent_methods_round_trip_over_socket() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let bin_dir = base.join("bin");
    write_fake_agent(&bin_dir, "pi", "printf 'Working...\\n'\nsleep 30\n");
    write_fake_agent(&bin_dir, "codex", "sleep 30\n");

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let path_override = format!("{}:{}", bin_dir.display(), inherited_path);
    let child = spawn_herdr_with_path(
        &config_home,
        &runtime_dir,
        &socket_path,
        Path::new(&path_override),
    );
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"agent_ws","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let terminal_id = created["result"]["root_pane"]["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();

    let renamed = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"agent_rename_pane","method":"pane.rename","params":{{"pane_id":"{}","label":"worker"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(renamed["result"]["pane"]["label"], "worker");

    start_pane_command(&socket_path, &pane_id, "pi");
    wait_for_pane_agent(&socket_path, &pane_id, "pi", "working");

    let listed = send_request(
        &socket_path,
        r#"{"id":"agent_list","method":"agent.list","params":{}}"#,
    );
    let agents = listed["result"]["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["terminal_id"], terminal_id);
    assert!(agents[0].get("name").is_none());
    assert_eq!(agents[0]["agent"], "pi");
    assert_eq!(agents[0]["agent_status"], "working");
    assert_eq!(agents[0]["pane_id"], pane_id);

    let fetched_by_pane = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"agent_get_pane","method":"agent.get","params":{{"target":"{}"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(
        fetched_by_pane["result"]["agent"]["terminal_id"],
        terminal_id
    );

    let renamed_first_agent = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"agent_rename_first","method":"agent.rename","params":{{"target":"{}","name":"worker"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(renamed_first_agent["result"]["agent"]["name"], "worker");

    let fetched_by_name = send_request(
        &socket_path,
        r#"{"id":"agent_get_name","method":"agent.get","params":{"target":"worker"}}"#,
    );
    assert_eq!(fetched_by_name["result"]["agent"]["name"], "worker");

    let read = send_request(
        &socket_path,
        r#"{"id":"agent_read","method":"agent.read","params":{"target":"worker","source":"visible"}}"#,
    );
    assert_eq!(read["result"]["type"], "pane_read");
    assert_eq!(read["result"]["read"]["pane_id"], pane_id);

    let sent = send_request(
        &socket_path,
        r#"{"id":"agent_send_keys","method":"agent.send_keys","params":{"target":"worker","keys":["enter"]}}"#,
    );
    assert_eq!(sent["result"]["type"], "ok", "{sent}");

    let tab_created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"agent_tab","method":"tab.create","params":{{"workspace_id":"{}","focus":false}}}}"#,
            workspace_id
        ),
    );
    let second_tab_id = tab_created["result"]["tab"]["tab_id"].as_str().unwrap();
    let second_pane_id = tab_created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap();
    let second_terminal_id = tab_created["result"]["root_pane"]["terminal_id"]
        .as_str()
        .unwrap();

    start_pane_command(&socket_path, second_pane_id, "codex");
    wait_for_pane_agent(&socket_path, second_pane_id, "codex", "idle");

    let second_renamed = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"agent_second_rename","method":"agent.rename","params":{{"target":"{}","name":"reviewer"}}}}"#,
            second_pane_id
        ),
    );
    assert_eq!(second_renamed["result"]["agent"]["name"], "reviewer");

    let duplicate = send_request(
        &socket_path,
        r#"{"id":"agent_duplicate","method":"agent.rename","params":{"target":"reviewer","name":"worker"}}"#,
    );
    assert_eq!(duplicate["error"]["code"], "agent_name_taken");
    assert!(duplicate["error"]["message"]
        .as_str()
        .unwrap()
        .contains(&terminal_id));

    let agent_renamed = send_request(
        &socket_path,
        r#"{"id":"agent_rename","method":"agent.rename","params":{"target":"reviewer","name":"qa"}}"#,
    );
    assert_eq!(agent_renamed["result"]["agent"]["name"], "qa");

    let focused = send_request(
        &socket_path,
        r#"{"id":"agent_focus","method":"agent.focus","params":{"target":"qa"}}"#,
    );
    assert_eq!(
        focused["result"]["agent"]["terminal_id"],
        second_terminal_id
    );
    assert_eq!(focused["result"]["agent"]["tab_id"], second_tab_id);
    assert_eq!(focused["result"]["agent"]["focused"], true);

    cleanup_spawned_herdr(child, base);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn reported_agent_session_clears_after_confirmed_process_exit() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let bin_dir = base.join("bin");

    fs::create_dir_all(&bin_dir).unwrap();
    let fake_pi = bin_dir.join("pi");
    let stop_file = base.join("pi-stop");
    fs::write(
        &fake_pi,
        format!(
            "#!/bin/sh\nprintf 'Working...\\n'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n",
            stop_file.display()
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
    let child = spawn_herdr_with_path(
        &config_home,
        &runtime_dir,
        &socket_path,
        Path::new(&path_override),
    );
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_release_1","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();

    let send_pi = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_release_2","method":"pane.send_text","params":{{"pane_id":"{}","text":"pi"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_pi["result"]["type"], "ok");
    let send_enter = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_release_3","method":"pane.send_keys","params":{{"pane_id":"{}","keys":["Enter"]}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_enter["result"]["type"], "ok");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let pane = send_request(
            &socket_path,
            &format!(
                r#"{{"id":"req_release_detect","method":"pane.get","params":{{"pane_id":"{}"}}}}"#,
                pane_id
            ),
        );
        if pane["result"]["pane"]["agent"] == "pi" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "pi agent was never detected: {pane}"
        );
        thread::sleep(Duration::from_millis(100));
    }

    let session_path = base.join("release-session.jsonl");
    let session = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_release_session","method":"pane.report_agent_session","params":{{"pane_id":"{}","source":"herdr:pi","agent":"pi","agent_session_path":"{}","session_start_source":"startup","seq":1}}}}"#,
            pane_id,
            session_path.display()
        ),
    );
    assert_eq!(session["result"]["type"], "ok");
    let observation_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < observation_deadline {
        let pane = send_request(
            &socket_path,
            &format!(
                r#"{{"id":"req_release_6","method":"pane.get","params":{{"pane_id":"{}"}}}}"#,
                pane_id
            ),
        );
        assert_eq!(
            pane["result"]["pane"]["agent"], "pi",
            "a session report hid the live Pi process: {pane}"
        );
        thread::sleep(Duration::from_millis(50));
    }

    fs::write(&stop_file, "stop").unwrap();

    let cleared_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let pane = send_request(
            &socket_path,
            &format!(
                r#"{{"id":"req_release_7","method":"pane.get","params":{{"pane_id":"{}"}}}}"#,
                pane_id
            ),
        );
        if pane["result"]["pane"]["agent"].is_null()
            && pane["result"]["pane"]["agent_status"] == "unknown"
        {
            break;
        }
        assert!(
            Instant::now() < cleared_deadline,
            "pi agent was not cleared promptly after process exit: {pane}"
        );
        thread::sleep(Duration::from_millis(50));
    }

    cleanup_spawned_herdr(child, base);
}
