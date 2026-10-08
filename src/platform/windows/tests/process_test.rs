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
fn windows_process_tree_selects_direct_agent_descendant() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
    ];
    let snapshot = super::super::ProcessSnapshot::new(entries);

    let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| panic!("Git Bash fallback must not run after normal detection succeeds"),
        |_| panic!("runtime marker must not be read after normal detection succeeds"),
    )
    .unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes.len(), 1);
    assert_eq!(job.processes[0].name, "codex.exe");
}

#[test]
fn windows_process_tree_still_inspects_unusual_escaped_argv0() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(20, 99, "launcher.exe", &["codex.exe"]),
    ]);

    let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| true,
        |_| Some("pane-a".to_string()),
    )
    .unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "launcher.exe");
}

#[test]
fn windows_process_tree_still_inspects_unusual_descendant_argv0() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "launcher.exe", &["codex.exe"]),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "launcher.exe");
}

#[test]
fn windows_process_tree_shares_snapshot_candidates_across_git_bash_panes() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(
            11,
            10,
            "bash.exe",
            &[r"C:\Program Files\Git\usr\bin\bash.exe"],
        ),
        test_entry(12, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(
            20,
            99,
            "sh.exe",
            &[r"C:\Program Files\Git\usr\bin\sh.exe", "/c/npm/codex"],
        ),
        test_entry(
            30,
            20,
            "node.exe",
            &[
                r"C:\Program Files\nodejs\node.exe",
                r"C:\Users\user\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js",
            ],
        ),
        test_entry(
            40,
            30,
            "codex.exe",
            &[r"C:\npm\node_modules\@openai\codex\bin\codex.exe"],
        ),
        test_entry(50, 98, "claude.exe", &["claude.exe"]),
    ]);
    let mut inspected = Vec::new();
    assert!(snapshot.agent_indices.get().is_none());
    let marker = |entry: &super::super::WindowsProcessEntry| match entry.pid {
        12 | 50 => Some("pane-b".to_string()),
        _ => Some("pane-a".to_string()),
    };

    let first = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| true,
        |entry| {
            inspected.push(entry.pid);
            marker(entry)
        },
    )
    .unwrap();
    let indices = snapshot.agent_indices.get().unwrap();
    let second = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        12,
        &snapshot,
        |_| true,
        marker,
    )
    .unwrap();

    assert_eq!(first.process_group_id, 20);
    assert_eq!(first.processes[0].name, "sh.exe");
    assert_eq!(second.process_group_id, 50);
    assert_eq!(indices, &[3, 4, 5, 6]);
    assert!(std::ptr::eq(indices, snapshot.agent_indices.get().unwrap()));
    assert_eq!(inspected, vec![10, 20, 30, 40, 50]);
}

#[test]
fn windows_process_tree_skips_runtime_inspection_for_non_git_bash_shell() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 99, "codex.exe", &["codex.exe"]),
    ];
    let snapshot = super::super::ProcessSnapshot::new(entries);

    let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| false,
        |_| panic!("runtime marker must not be read for non-Git-Bash panes"),
    )
    .unwrap();

    assert_eq!(job.process_group_id, 10);
}

#[test]
fn windows_process_tree_skips_runtime_inspection_without_agent_candidate() {
    let entries = vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(20, 99, "git.exe", &["git.exe", "status"]),
    ];
    let snapshot = super::super::ProcessSnapshot::new(entries);

    let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| true,
        |_| panic!("runtime marker must not be read without an agent candidate"),
    )
    .unwrap();

    assert_eq!(job.process_group_id, 10);
}

#[test]
fn windows_process_tree_rejects_missing_or_empty_shell_runtime_marker() {
    let entries = vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(20, 99, "codex.exe", &["codex.exe"]),
    ];
    let snapshot = super::super::ProcessSnapshot::new(entries);

    for shell_marker in [None, Some(String::new())] {
        let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
            10,
            &snapshot,
            |_| true,
            |entry| {
                if entry.pid == 10 {
                    shell_marker.clone()
                } else {
                    Some("pane-a".to_string())
                }
            },
        )
        .unwrap();

        assert_eq!(job.process_group_id, 10);
    }
}

#[test]
fn windows_process_tree_rejects_runtime_marker_from_another_pane() {
    let entries = vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(20, 99, "codex.exe", &["codex.exe"]),
    ];
    let snapshot = super::super::ProcessSnapshot::new(entries);

    let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| true,
        |entry| Some(if entry.pid == 10 { "pane-a" } else { "pane-b" }.to_string()),
    )
    .unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "bash.exe");
}

#[test]
fn windows_process_tree_rejects_ambiguous_runtime_marker_candidates() {
    let entries = vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(20, 99, "codex.exe", &["codex.exe"]),
        test_entry(30, 98, "claude.exe", &["claude.exe"]),
    ];
    let snapshot = super::super::ProcessSnapshot::new(entries);

    let job = super::super::select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        10,
        &snapshot,
        |_| true,
        |_| Some("pane-a".to_string()),
    )
    .unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "bash.exe");
}

#[test]
fn windows_process_tree_selects_wrapped_agent_descendant() {
    let entries = vec![
        test_entry(10, 1, "cmd.exe", &["cmd.exe"]),
        test_entry(
            20,
            10,
            "node.exe",
            &[
                "node.exe",
                "C:\\Users\\herdr\\AppData\\Roaming\\npm\\node_modules\\codex\\bin\\codex.js",
            ],
        ),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "node.exe");
}

#[test]
fn windows_process_tree_selects_cmd_wrapped_agent_descendant() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(
            20,
            10,
            "cmd.exe",
            &[
                "cmd.exe",
                "/D",
                "/S",
                "/C",
                "C:\\Users\\herdr\\AppData\\Roaming\\npm\\codex.cmd --model gpt-5",
            ],
        ),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "cmd.exe");
}

#[test]
fn windows_process_tree_selects_topmost_codex_process_in_single_agent_chain() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(
            20,
            10,
            "node.exe",
            &[
                "node.exe",
                "C:\\Users\\herdr\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\bin\\codex.js",
            ],
        ),
        test_entry(
            30,
            20,
            "codex.exe",
            &["C:\\Users\\herdr\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\node_modules\\@openai\\codex-win32-x64\\vendor\\x86_64-pc-windows-msvc\\bin\\codex.exe"],
        ),
        test_entry(40, 30, "node_repl.exe", &["node_repl.exe"]),
        test_entry(
            50,
            40,
            "codex.exe",
            &["codex.exe", "app-server", "--listen", "stdio://"],
        ),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "node.exe");
}

#[test]
fn windows_process_tree_keeps_topmost_agent_over_different_agent_descendant() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "claude.exe", &["claude.exe"]),
        test_entry(
            30,
            20,
            "cmd.exe",
            &["cmd.exe", "/D", "/S", "/C", "codex mcp-server"],
        ),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "claude.exe");
}

#[test]
fn windows_process_tree_keeps_root_agent_over_agent_descendant() {
    let entries = vec![
        test_entry(10, 1, "claude.exe", &["claude.exe"]),
        test_entry(
            20,
            10,
            "cmd.exe",
            &["cmd.exe", "/D", "/S", "/C", "codex mcp-server"],
        ),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "claude.exe");
}

#[test]
fn windows_process_tree_returns_shell_for_same_agent_siblings() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "codex.exe", &["codex.exe"]),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
}

#[test]
fn windows_process_tree_returns_shell_for_plain_descendant() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "git.exe", &["git.exe", "status"]),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
}

#[test]
fn windows_process_tree_returns_shell_for_multiple_agent_descendants() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "claude.exe", &["claude.exe"]),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
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
fn windows_process_tree_returns_shell_when_candidate_parent_chain_cycles() {
    let entries = vec![
        test_entry(10, 40, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "codex.exe", &["codex.exe"]),
        test_entry(40, 10, "node.exe", &["node.exe"]),
    ];

    let job = super::super::select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
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
