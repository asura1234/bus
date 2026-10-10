use super::*;

#[test]
fn windows_process_cwd_reads_child_launch_directory() {
    let cwd = std::env::temp_dir().join(format!("bus-cwd-test-{}", std::process::id()));
    fs::create_dir_all(&cwd).expect("create cwd fixture");

    let shell =
        std::env::var_os("ComSpec").unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into());
    let mut child = Command::new(shell)
        .args(["/D", "/Q", "/C", "ping -n 11 127.0.0.1 > NUL"])
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut observed = None;
    while Instant::now() < deadline {
        observed = super::super::process_cwd(child.id());
        if observed.as_deref() == Some(cwd.as_path()) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(&cwd);

    assert_eq!(observed.as_deref(), Some(cwd.as_path()));
}

#[test]
fn windows_process_environment_reads_runtime_marker() {
    let shell =
        std::env::var_os("ComSpec").unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into());
    let mut child = Command::new(shell)
        .args(["/D", "/Q", "/C", "ping -n 11 127.0.0.1 > NUL"])
        .env(super::super::PANE_RUNTIME_MARKER_ENV_VAR, "pane-test")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut observed = None;
    while Instant::now() < deadline {
        observed = super::super::process_runtime_marker(child.id());
        if observed.as_deref() == Some("pane-test") {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(observed.as_deref(), Some("pane-test"));
}

#[test]
fn windows_session_processes_collects_shell_and_descendants() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "cmd.exe", &["cmd.exe"]),
        test_entry(30, 20, "node.exe", &["node.exe"]),
        test_entry(40, 1, "unrelated.exe", &["unrelated.exe"]),
    ];

    let snapshot = super::super::ProcessSnapshot::new(entries);
    let mut pids = super::super::session_processes_from_snapshot(10, &snapshot);
    pids.sort_unstable();

    assert_eq!(pids, vec![10, 20, 30]);
}

#[test]
fn windows_process_tree_ignores_pid_reuse_cycles() {
    let entries = vec![
        test_entry(10, 30, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 20, "node.exe", &["node.exe"]),
    ];

    let snapshot = super::super::ProcessSnapshot::new(entries);
    let descendants = super::super::descendant_entries(10, &snapshot);

    assert_eq!(
        descendants
            .iter()
            .map(|entry| entry.pid)
            .collect::<Vec<_>>(),
        vec![20, 30]
    );
}

#[test]
fn process_environment_variable_parser_reads_case_insensitive_marker() {
    let environment: Vec<u16> = "PATH=C:\\Windows\0herdr_pane_runtime_id=pane-a\0\0"
        .encode_utf16()
        .collect();

    assert_eq!(
        super::super::environment_variable_from_utf16(
            &environment,
            super::super::PANE_RUNTIME_MARKER_ENV_VAR,
        )
        .as_deref(),
        Some("pane-a")
    );
}

#[test]
fn pane_runtime_markers_are_distinct() {
    let first = super::super::next_pane_runtime_marker();
    let second = super::super::next_pane_runtime_marker();

    assert_ne!(first, second);
}

#[test]
fn pane_runtime_marker_is_added_only_to_git_bash_environment() {
    let root = std::env::temp_dir().join(format!(
        "bus-git-bash-test-{}",
        super::super::next_pane_runtime_marker()
    ));
    fs::create_dir_all(root.join("bin")).expect("create Git Bash bin fixture");
    fs::create_dir_all(root.join("usr").join("bin")).expect("create Git Bash usr/bin fixture");
    fs::create_dir_all(root.join("cmd")).expect("create Git Bash cmd fixture");
    fs::write(root.join("bin").join("bash.exe"), []).expect("create Bash fixture");
    fs::write(root.join("usr").join("bin").join("msys-2.0.dll"), [])
        .expect("create MSYS runtime fixture");
    fs::write(root.join("cmd").join("git.exe"), []).expect("create Git fixture");

    let mut git_bash = portable_pty::CommandBuilder::new(root.join("bin").join("bash.exe"));
    super::super::apply_pane_runtime_marker_platform(&mut git_bash);
    let mut path_resolved_git_bash = portable_pty::CommandBuilder::new("bash.exe");
    path_resolved_git_bash.env("PATH", root.join("bin"));
    super::super::apply_pane_runtime_marker_platform(&mut path_resolved_git_bash);
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    super::super::apply_pane_runtime_marker_platform(&mut cmd);

    assert!(git_bash
        .get_env(super::super::PANE_RUNTIME_MARKER_ENV_VAR)
        .is_some_and(|value| !value.is_empty()));
    assert!(path_resolved_git_bash
        .get_env(super::super::PANE_RUNTIME_MARKER_ENV_VAR)
        .is_some_and(|value| !value.is_empty()));
    assert!(cmd
        .get_env(super::super::PANE_RUNTIME_MARKER_ENV_VAR)
        .is_none());
    fs::remove_dir_all(root).expect("remove Git Bash fixture");
}

#[test]
fn windows_signal_processes_terminates_running_process_on_kill() {
    assert_windows_signal_terminates_process(crate::platform::Signal::Kill);
}

#[test]
fn windows_signal_processes_terminates_running_process_on_terminate() {
    assert_windows_signal_terminates_process(crate::platform::Signal::Terminate);
}

fn assert_windows_signal_terminates_process(signal: crate::platform::Signal) {
    let shell =
        std::env::var_os("ComSpec").unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into());
    let mut child = Command::new(shell)
        .args(["/D", "/Q", "/C", "ping -n 30 127.0.0.1 > NUL"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd");
    let pid = child.id();
    assert!(super::super::process_exists(pid));

    super::super::signal_processes(&[pid], crate::platform::Signal::Hangup);
    assert!(super::super::process_exists(pid), "Hangup remains a no-op");
    super::super::signal_processes(&[pid], signal);

    let deadline = Instant::now() + Duration::from_secs(5);
    while super::super::process_exists(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    let still_running = super::super::process_exists(pid);
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        !still_running,
        "{signal:?} must terminate the process it was given"
    );
}

#[test]
fn windows_session_processes_excludes_orphan_created_before_reused_shell_pid() {
    let entries = vec![
        // The orphan was spawned by an earlier process that also had pid 10 and has exited;
        // the toolhelp snapshot still reports that stale parent id.
        test_entry_with_creation_time(20, 10, "codex.exe", &["codex.exe"], Some(100)),
        test_entry_with_creation_time(10, 1, "powershell.exe", &["powershell.exe"], Some(200)),
        test_entry_with_creation_time(30, 10, "node.exe", &["node.exe"], Some(300)),
    ];

    let snapshot = super::super::ProcessSnapshot::new(entries);
    let mut pids = super::super::session_processes_from_snapshot(10, &snapshot);
    pids.sort_unstable();

    assert_eq!(pids, vec![10, 30]);
}

#[test]
fn windows_session_processes_excludes_stale_nested_parent_links_and_their_subtrees() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry_with_creation_time(10, 1, "powershell.exe", &[], Some(100)),
        test_entry_with_creation_time(20, 10, "cmd.exe", &[], Some(200)),
        test_entry_with_creation_time(30, 20, "orphan.exe", &[], Some(150)),
        test_entry_with_creation_time(40, 30, "orphan-child.exe", &[], Some(400)),
        test_entry_with_creation_time(50, 20, "node.exe", &[], Some(300)),
        test_entry_with_creation_time(60, 10, "cmd.exe", &[], Some(300)),
    ]);

    assert_eq!(
        super::super::session_processes_from_snapshot(10, &snapshot),
        vec![10, 20, 60, 50]
    );
}

#[test]
fn windows_session_processes_preserves_equal_and_unknown_creation_times() {
    for (parent_time, child_time) in [
        (Some(200), Some(200)),
        (Some(200), None),
        (None, Some(100)),
        (None, None),
    ] {
        let snapshot = super::super::ProcessSnapshot::new(vec![
            test_entry_with_creation_time(10, 1, "powershell.exe", &[], parent_time),
            test_entry_with_creation_time(20, 10, "cmd.exe", &[], child_time),
        ]);

        assert_eq!(
            super::super::session_processes_from_snapshot(10, &snapshot),
            vec![10, 20],
            "parent time {parent_time:?}, child time {child_time:?}"
        );
    }
}

#[test]
fn windows_available_pane_shell_ignores_stale_orphan_subtree() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry_with_creation_time(10, 1, "powershell.exe", &[], Some(200)),
        test_entry_with_creation_time(20, 10, "codex.exe", &[], Some(100)),
        test_entry_with_creation_time(30, 20, "node.exe", &[], Some(300)),
    ]);

    assert_eq!(
        super::super::session_processes_from_snapshot(10, &snapshot),
        vec![10]
    );
    assert_eq!(
        super::super::available_pane_shell_from_snapshot(10, &snapshot).as_deref(),
        Some("powershell.exe")
    );
    assert!(super::super::session_processes_from_snapshot(99, &snapshot).is_empty());
    // The raw walk still accepts an unavailable root, whose age cannot be checked.
    assert_eq!(super::super::descendant_entries(1, &snapshot).len(), 1);
}
