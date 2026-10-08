use super::*;

#[test]
fn windows_process_cwd_reads_child_launch_directory() {
    let cwd = std::env::temp_dir().join(format!("herdr-cwd-test-{}", std::process::id()));
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
        "herdr-git-bash-test-{}",
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
