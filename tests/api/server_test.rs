use super::*;

#[test]
fn ping_over_socket_returns_version() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let child = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let value = send_request(
        &socket_path,
        r#"{"id":"req_1","method":"ping","params":{}}"#,
    );
    assert_eq!(value["id"], "req_1");
    assert_eq!(value["result"]["type"], "pong");
    assert_eq!(value["result"]["version"], env!("CARGO_PKG_VERSION"));
    // Intentionally hardcoded so wire protocol bumps require updating this test.
    // Changing this value means old clients/servers are no longer compatible.
    assert_eq!(value["result"]["protocol"], 22);

    cleanup_spawned_herdr(child, base);
}

#[cfg(target_os = "linux")]
#[test]
fn shutdown_preserves_session_after_shell_is_signaled() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let mut child = spawn_herdr_with_shell(&config_home, &runtime_dir, &socket_path, "/bin/sh");
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"create","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id");
    let pid_file = base.join("shell.pid");
    let pid_command = format!(
        "printf %s $$ > {path}.tmp && mv {path}.tmp {path}",
        path = pid_file.display()
    );
    let sent = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "shell_pid",
            "method": "pane.send_input",
            "params": {
                "pane_id": pane_id,
                "text": pid_command,
                "keys": ["Enter"],
            },
        })
        .to_string(),
    );
    assert_eq!(sent["result"]["type"], "ok");
    wait_for_path(&pid_file, Duration::from_secs(5));
    let shell_pid: libc::pid_t = fs::read_to_string(&pid_file)
        .unwrap()
        .parse()
        .expect("shell pid");

    assert_eq!(unsafe { libc::kill(shell_pid, libc::SIGHUP) }, 0);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let panes = send_request(
            &socket_path,
            r#"{"id":"panes","method":"pane.list","params":{}}"#,
        );
        if panes["result"]["panes"]
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            break;
        }
        assert!(Instant::now() < deadline, "signaled pane was not removed");
        thread::sleep(Duration::from_millis(20));
    }

    let stopped = send_request(
        &socket_path,
        r#"{"id":"stop","method":"server.stop","params":{}}"#,
    );
    assert_eq!(stopped["result"]["type"], "ok");
    child.child.wait().expect("server should stop cleanly");

    let session: serde_json::Value = serde_json::from_slice(
        &fs::read(config_home.join("herdr-dev/session.json")).expect("saved session"),
    )
    .expect("valid session json");
    assert_eq!(session["workspaces"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        session["workspaces"][0]["tabs"][0]["panes"]
            .as_object()
            .map(serde_json::Map::len),
        Some(1)
    );

    cleanup_spawned_herdr(child, base);
}
