use super::*;

#[cfg(not(target_os = "macos"))]
#[test]
fn workspace_list_and_create_round_trip() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let child = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let empty = send_request(
        &socket_path,
        r#"{"id":"req_2","method":"workspace.list","params":{}}"#,
    );
    assert_eq!(empty["id"], "req_2");
    assert_eq!(empty["result"]["type"], "workspace_list");
    assert_eq!(empty["result"]["workspaces"].as_array().unwrap().len(), 0);

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_3","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    assert_eq!(created["id"], "req_3");
    assert_eq!(created["result"]["type"], "workspace_created");
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let active_tab_id = created["result"]["workspace"]["active_tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    let root_pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let root_terminal_id = created["result"]["root_pane"]["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(root_terminal_id.starts_with("term_"));
    assert_ne!(root_terminal_id, root_pane_id);
    assert_eq!(created["result"]["workspace"]["number"], 1);
    assert_eq!(created["result"]["workspace"]["focused"], true);
    assert_eq!(created["result"]["workspace"]["tab_count"], 1);
    assert_eq!(created["result"]["tab"]["tab_id"], active_tab_id);
    assert_eq!(created["result"]["root_pane"]["tab_id"], active_tab_id);
    assert_eq!(active_tab_id, format!("{workspace_id}:t1"));

    let listed = send_request(
        &socket_path,
        r#"{"id":"req_4","method":"workspace.list","params":{}}"#,
    );
    let workspaces = listed["result"]["workspaces"].as_array().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["workspace_id"], workspace_id);

    let fetched = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_5","method":"workspace.get","params":{{"workspace_id":"{}"}}}}"#,
            workspace_id
        ),
    );
    assert_eq!(fetched["result"]["workspace"]["workspace_id"], workspace_id);

    let panes = send_request(
        &socket_path,
        r#"{"id":"req_6","method":"pane.list","params":{}}"#,
    );
    let panes = panes["result"]["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0]["workspace_id"], workspace_id);
    assert_eq!(panes[0]["tab_id"], active_tab_id);
    let pane_id = panes[0]["pane_id"].as_str().unwrap().to_string();
    assert_eq!(pane_id, root_pane_id);
    assert_eq!(panes[0]["terminal_id"], root_terminal_id);
    let legacy_pane_id = format!("{workspace_id}-1");

    let pane = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_7","method":"pane.get","params":{{"pane_id":"{}"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(pane["result"]["pane"]["pane_id"], pane_id);
    assert_eq!(pane["result"]["pane"]["terminal_id"], root_terminal_id);

    let read = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_8","method":"pane.read","params":{{"pane_id":"{}","source":"visible"}}}}"#,
            legacy_pane_id
        ),
    );
    assert_eq!(read["result"]["read"]["pane_id"], pane_id);
    assert_eq!(read["result"]["read"]["tab_id"], active_tab_id);
    assert!(read["result"]["read"]["text"].is_string());

    let send_text = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_9","method":"pane.send_text","params":{{"pane_id":"{}","text":"echo alpha; echo beta; echo gamma"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_text["result"]["type"], "ok");

    let send_enter = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_10","method":"pane.send_keys","params":{{"pane_id":"{}","keys":["Enter"]}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_enter["result"]["type"], "ok");

    std::thread::sleep(Duration::from_millis(300));

    let recent = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_11","method":"pane.read","params":{{"pane_id":"{}","source":"recent","lines":50}}}}"#,
            pane_id
        ),
    );
    let recent_text = recent["result"]["read"]["text"].as_str().unwrap();
    assert!(recent_text.contains("beta") || recent_text.contains("gamma"));

    for source in ["visible", "detection"] {
        let limited = send_request(
            &socket_path,
            &format!(
                r#"{{"id":"req_11_{source}","method":"pane.read","params":{{"pane_id":"{}","source":"{source}","lines":2}}}}"#,
                pane_id
            ),
        );
        let text = limited["result"]["read"]["text"].as_str().unwrap();
        assert!(
            text.lines().count() <= 2,
            "{source} ignored its two-line limit: {text:?}"
        );
    }

    let waited = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_12","method":"pane.wait_for_output","params":{{"pane_id":"{}","source":"recent","lines":40,"match":{{"type":"substring","value":"gamma"}},"timeout_ms":2000}}}}"#,
            legacy_pane_id
        ),
    );
    assert_eq!(waited["result"]["type"], "output_matched");
    assert_eq!(waited["result"]["pane_id"], pane_id);
    assert_eq!(waited["result"]["read"]["pane_id"], pane_id);
    assert!(waited["result"]["matched_line"]
        .as_str()
        .unwrap()
        .contains("gamma"));
    assert!(waited["result"]["read"]["text"]
        .as_str()
        .unwrap()
        .contains("gamma"));

    let send_input = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_12b","method":"pane.send_input","params":{{"pane_id":"{}","text":"echo delta","keys":["Enter"]}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_input["result"]["type"], "ok");

    let waited_delta = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_12c","method":"pane.wait_for_output","params":{{"pane_id":"{}","source":"recent","lines":40,"match":{{"type":"substring","value":"delta"}},"timeout_ms":2000}}}}"#,
            pane_id
        ),
    );
    assert_eq!(waited_delta["result"]["type"], "output_matched");
    assert_eq!(waited_delta["result"]["pane_id"], pane_id);
    assert_eq!(waited_delta["result"]["read"]["pane_id"], pane_id);
    assert!(waited_delta["result"]["matched_line"]
        .as_str()
        .unwrap()
        .contains("delta"));

    let waited_regex = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_13","method":"pane.wait_for_output","params":{{"pane_id":"{}","source":"recent","lines":40,"match":{{"type":"regex","value":"alp.*gamma"}},"timeout_ms":2000}}}}"#,
            pane_id
        ),
    );
    assert_eq!(waited_regex["result"]["type"], "output_matched");
    assert_eq!(waited_regex["result"]["pane_id"], pane_id);
    assert_eq!(waited_regex["result"]["read"]["pane_id"], pane_id);
    assert!(waited_regex["result"]["matched_line"]
        .as_str()
        .unwrap()
        .contains("alpha"));

    let timeout = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_14","method":"pane.wait_for_output","params":{{"pane_id":"{}","source":"recent","lines":10,"match":{{"type":"substring","value":"definitely-not-there"}},"timeout_ms":200}}}}"#,
            pane_id
        ),
    );
    assert_eq!(timeout["error"]["code"], "timeout");

    cleanup_spawned_herdr(child, base);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn tab_methods_round_trip_over_socket() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let child = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t1","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_tab_id = created["result"]["workspace"]["active_tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(first_tab_id, format!("{workspace_id}:t1"));

    let tab_created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t2","method":"tab.create","params":{{"workspace_id":"{}","focus":true}}}}"#,
            workspace_id
        ),
    );
    assert_eq!(tab_created["result"]["type"], "tab_created");
    let second_tab_id = tab_created["result"]["tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    let second_root_pane_id = tab_created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let second_root_terminal_id = tab_created["result"]["root_pane"]["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(second_root_terminal_id.starts_with("term_"));
    assert_ne!(second_root_terminal_id, second_root_pane_id);
    assert_eq!(second_tab_id, format!("{workspace_id}:t2"));
    assert_eq!(tab_created["result"]["tab"]["focused"], true);
    assert_eq!(tab_created["result"]["root_pane"]["tab_id"], second_tab_id);

    let tab_list = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t3","method":"tab.list","params":{{"workspace_id":"{}"}}}}"#,
            workspace_id
        ),
    );
    let tabs = tab_list["result"]["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0]["tab_id"], first_tab_id);

    let panes = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t3b","method":"pane.list","params":{{"workspace_id":"{}"}}}}"#,
            workspace_id
        ),
    );
    let panes = panes["result"]["panes"].as_array().unwrap();
    assert!(panes.iter().any(|pane| {
        pane["pane_id"] == second_root_pane_id && pane["terminal_id"] == second_root_terminal_id
    }));
    assert_eq!(tabs[1]["tab_id"], second_tab_id);

    let tab_get = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t4","method":"tab.get","params":{{"tab_id":"{}"}}}}"#,
            second_tab_id
        ),
    );
    assert_eq!(tab_get["result"]["tab"]["tab_id"], second_tab_id);

    let renamed = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t5","method":"tab.rename","params":{{"tab_id":"{}","label":"logs"}}}}"#,
            second_tab_id
        ),
    );
    assert_eq!(renamed["result"]["tab"]["label"], "logs");

    let focused = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t6","method":"tab.focus","params":{{"tab_id":"{}"}}}}"#,
            first_tab_id
        ),
    );
    assert_eq!(focused["result"]["tab"]["tab_id"], first_tab_id);
    assert_eq!(focused["result"]["tab"]["focused"], true);

    let closed = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_t7","method":"tab.close","params":{{"tab_id":"{}"}}}}"#,
            second_tab_id
        ),
    );
    assert_eq!(closed["result"]["type"], "ok");

    cleanup_spawned_herdr(child, base);
}

#[test]
fn tab_create_with_no_focus_preserves_active_tab() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let child = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_nf_1","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_tab_id = created["result"]["tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_string();

    let tab_created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_nf_2","method":"tab.create","params":{{"workspace_id":"{}","focus":false}}}}"#,
            workspace_id
        ),
    );
    assert_eq!(tab_created["result"]["type"], "tab_created");
    let second_tab_id = tab_created["result"]["tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(second_tab_id, format!("{workspace_id}:t2"));
    assert_eq!(tab_created["result"]["tab"]["focused"], false);

    let tab_list = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_nf_3","method":"tab.list","params":{{"workspace_id":"{}"}}}}"#,
            workspace_id
        ),
    );
    let tabs = tab_list["result"]["tabs"].as_array().unwrap();
    assert_eq!(tabs[0]["tab_id"], first_tab_id);
    assert_eq!(tabs[0]["focused"], true);
    assert_eq!(tabs[1]["tab_id"], second_tab_id);
    assert_eq!(tabs[1]["focused"], false);

    cleanup_spawned_herdr(child, base);
}
