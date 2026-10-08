use super::*;

#[cfg(target_os = "linux")]
#[test]
fn pane_info_reports_foreground_cwd_without_changing_pane_cwd() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let foreground = base.join("foreground-process");
    let marker = base.join("foreground-ready");
    let pid_file = base.join("foreground.pid");
    fs::create_dir_all(&foreground).unwrap();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let child = spawn_herdr_with_shell(&config_home, &runtime_dir, &socket_path, "/bin/bash");
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"fg_ws","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let command = format!(
        "/bin/sh -c 'cd {} && printf %s $$ > {} && touch {} && sleep 30; :'",
        foreground.display(),
        pid_file.display(),
        marker.display()
    );
    let send_text = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "fg_send",
            "method": "pane.send_text",
            "params": {
                "pane_id": pane_id,
                "text": command,
            },
        })
        .to_string(),
    );
    assert_eq!(send_text["result"]["type"], "ok");
    let send_enter = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"fg_enter","method":"pane.send_keys","params":{{"pane_id":"{}","keys":["Enter"]}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_enter["result"]["type"], "ok");
    wait_for_path(&marker, Duration::from_secs(5));

    let foreground_pid: u32 = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    assert_eq!(
        fs::read_link(format!("/proc/{foreground_pid}/cwd")).unwrap(),
        foreground
    );

    let pane = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"fg_pane","method":"pane.get","params":{{"pane_id":"{}"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(pane["result"]["pane"]["cwd"], base.display().to_string());
    assert_eq!(
        pane["result"]["pane"]["foreground_cwd"],
        foreground.display().to_string()
    );

    let panes = send_request(
        &socket_path,
        r#"{"id":"fg_panes","method":"pane.list","params":{}}"#,
    );
    assert_eq!(
        panes["result"]["panes"][0]["cwd"],
        base.display().to_string()
    );
    assert_eq!(
        panes["result"]["panes"][0]["foreground_cwd"],
        foreground.display().to_string()
    );

    let split = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "fg_split",
            "method": "pane.split",
            "params": {
                "target_pane_id": pane_id,
                "direction": "right",
                "focus": false,
            },
        })
        .to_string(),
    );
    assert_eq!(
        split["result"]["pane"]["cwd"],
        foreground.display().to_string()
    );

    cleanup_spawned_herdr(child, base);
}

#[cfg(target_os = "linux")]
#[test]
fn new_terminal_cwd_follow_ignores_nonleader_group_member_cwd() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let helper_cwd = base.join("plugin-cache");
    let helper_marker = base.join("helper-cwd-ready");
    let marker = base.join("helper-ready");
    let leader_pid_file = base.join("leader.pid");
    let helper_pid_file = base.join("helper.pid");
    fs::create_dir_all(&helper_cwd).unwrap();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let child = spawn_herdr_with_shell(&config_home, &runtime_dir, &socket_path, "/bin/bash");
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"member_ws","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let command = format!(
        "/bin/sh -c 'printf %s $$ > {}; (cd {} && touch {} && sleep 30) & printf %s $! > {}; while [ ! -e {} ]; do sleep 0.01; done; touch {}; wait'",
        leader_pid_file.display(),
        helper_cwd.display(),
        helper_marker.display(),
        helper_pid_file.display(),
        helper_marker.display(),
        marker.display()
    );
    let send_text = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "member_send",
            "method": "pane.send_text",
            "params": {
                "pane_id": pane_id,
                "text": command,
            },
        })
        .to_string(),
    );
    assert_eq!(send_text["result"]["type"], "ok");
    let send_enter = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"member_enter","method":"pane.send_keys","params":{{"pane_id":"{}","keys":["Enter"]}}}}"#,
            pane_id
        ),
    );
    assert_eq!(send_enter["result"]["type"], "ok");
    wait_for_path(&marker, Duration::from_secs(5));

    let leader_pid: u32 = fs::read_to_string(&leader_pid_file)
        .unwrap()
        .parse()
        .unwrap();
    let helper_pid: u32 = fs::read_to_string(&helper_pid_file)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::getpgid(leader_pid as libc::pid_t) },
        leader_pid as libc::pid_t
    );
    assert_eq!(
        unsafe { libc::getpgid(helper_pid as libc::pid_t) },
        leader_pid as libc::pid_t
    );
    assert_eq!(
        fs::read_link(format!("/proc/{leader_pid}/cwd")).unwrap(),
        base
    );
    assert_eq!(
        fs::read_link(format!("/proc/{helper_pid}/cwd")).unwrap(),
        helper_cwd
    );

    let pane = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"member_pane","method":"pane.get","params":{{"pane_id":"{}"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(pane["result"]["pane"]["cwd"], base.display().to_string());
    // Regression for issue #3270: the foreground group leader's cwd is
    // authoritative; the backgrounded helper's chdir must not override it.
    assert_eq!(
        pane["result"]["pane"]["foreground_cwd"],
        base.display().to_string()
    );

    let split = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "member_split",
            "method": "pane.split",
            "params": {
                "target_pane_id": pane_id,
                "direction": "right",
                "focus": false,
            },
        })
        .to_string(),
    );
    assert_eq!(split["result"]["pane"]["cwd"], base.display().to_string());

    let tab = send_request(
        &socket_path,
        &serde_json::json!({
            "id": "member_tab",
            "method": "tab.create",
            "params": {
                "workspace_id": workspace_id,
                "focus": false,
            },
        })
        .to_string(),
    );
    assert_eq!(
        tab["result"]["root_pane"]["cwd"],
        base.display().to_string()
    );

    cleanup_spawned_herdr(child, base);
}
