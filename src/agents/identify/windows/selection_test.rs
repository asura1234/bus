use super::*;
use crate::platform::WindowsProcessCommand;

fn test_entry(pid: u32, parent_pid: u32, name: &str, argv: &[&str]) -> WindowsProcessEntry {
    let command = OnceLock::new();
    command
        .set(WindowsProcessCommand {
            creation_time: None,
            argv0: argv.first().map(|value| (*value).to_string()),
            argv: Some(argv.iter().map(|value| (*value).to_string()).collect()),
            cmdline: Some(argv.join(" ")),
        })
        .unwrap();
    WindowsProcessEntry {
        pid,
        parent_pid,
        name: name.to_string(),
        command,
    }
}

#[test]
fn windows_process_tree_selects_direct_agent_descendant() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
    ];
    let snapshot = AgentProcessSnapshot::new(entries);

    let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let snapshot = AgentProcessSnapshot::new(vec![
        test_entry(10, 1, "bash.exe", &[r"C:\Program Files\Git\bin\bash.exe"]),
        test_entry(20, 99, "launcher.exe", &["codex.exe"]),
    ]);

    let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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

    let job = select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 20);
    assert_eq!(job.processes[0].name, "launcher.exe");
}

#[test]
fn windows_process_tree_shares_snapshot_candidates_across_git_bash_panes() {
    let snapshot = AgentProcessSnapshot::new(vec![
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
    let marker = |entry: &WindowsProcessEntry| match entry.pid {
        12 | 50 => Some("pane-b".to_string()),
        _ => Some("pane-a".to_string()),
    };

    let first = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let second = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let snapshot = AgentProcessSnapshot::new(entries);

    let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let snapshot = AgentProcessSnapshot::new(entries);

    let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let snapshot = AgentProcessSnapshot::new(entries);

    for shell_marker in [None, Some(String::new())] {
        let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let snapshot = AgentProcessSnapshot::new(entries);

    let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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
    let snapshot = AgentProcessSnapshot::new(entries);

    let job = select_pane_foreground_job_from_snapshot_with_runtime_inspection(
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

    let job = select_pane_foreground_job(10, &entries).unwrap();

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

    let job = select_pane_foreground_job(10, &entries).unwrap();

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

    let job = select_pane_foreground_job(10, &entries).unwrap();

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

    let job = select_pane_foreground_job(10, &entries).unwrap();

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

    let job = select_pane_foreground_job(10, &entries).unwrap();

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

    let job = select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
}

#[test]
fn windows_process_tree_returns_shell_for_plain_descendant() {
    let entries = vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "git.exe", &["git.exe", "status"]),
    ];

    let job = select_pane_foreground_job(10, &entries).unwrap();

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

    let job = select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
}

#[test]
fn windows_process_tree_returns_shell_when_candidate_parent_chain_cycles() {
    let entries = vec![
        test_entry(10, 40, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "codex.exe", &["codex.exe"]),
        test_entry(40, 10, "node.exe", &["node.exe"]),
    ];

    let job = select_pane_foreground_job(10, &entries).unwrap();

    assert_eq!(job.process_group_id, 10);
    assert_eq!(job.processes[0].name, "powershell.exe");
}
