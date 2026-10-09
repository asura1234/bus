use super::*;

fn foreground_process(pid: u32, name: &str, argv: &[&str]) -> crate::platform::ForegroundProcess {
    crate::platform::ForegroundProcess {
        pid,
        name: name.to_string(),
        argv0: None,
        argv: Some(argv.iter().map(|arg| (*arg).to_string()).collect()),
        cmdline: Some(argv.join(" ")),
    }
}

#[cfg(unix)]
fn temp_detection_path(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "herdr-detect-tests-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    );
    std::env::temp_dir().join(unique)
}

// ---- Agent identification ----

#[test]
fn identify_agent_in_job_prefers_wrapped_codex() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![
            foreground_process(1, "node", &["node", "/path/to/bin/codex"]),
            foreground_process(2, "bash", &["bash"]),
        ],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Codex, "codex".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_node_wrapped_qwen() {
    for argv in [
        vec!["node", "/home/user/.fnm/bin/qwen"],
        vec![
            "node.exe",
            r"C:\Users\user\AppData\Roaming\npm\node_modules\@qwen-code\qwen-code\dist\index.js",
        ],
    ] {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(123, "MainThread", &argv)],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((AgentKind::Qwen, "qwen".to_string()))
        );
    }
}

#[test]
fn identify_agent_in_job_detects_windows_cursor_install() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "node.exe",
            &[
                r"C:\Users\user\AppData\Local\cursor-agent\versions\2026.08.11-e8db854\node.exe",
                r"C:\Users\user\AppData\Local\cursor-agent\versions\2026.08.11-e8db854\index.js",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Cursor, "cursor".to_string()))
    );
}

#[test]
fn identify_agent_in_job_ignores_invalid_windows_cursor_install_paths() {
    for script in [
        r"C:\Users\user\AppData\Local\cursor-agent\versions\2026.08.11-e8db854\scripts\postinstall.js",
        r"C:\Users\user\AppData\Local\cursor-agent\versions\2026.08.11-e8db854\index",
        r"C:\Users\user\AppData\Local\cursor-agent\versions\2026.08.11-e8db854\index.exe",
    ] {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                123,
                "node.exe",
                &[
                    r"C:\Users\user\AppData\Local\cursor-agent\versions\2026.08.11-e8db854\node.exe",
                    script,
                ],
            )],
        };

        assert_eq!(identify_agent_in_job(&job), None, "script: {script}");
    }

    let lookalike = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "node.exe",
            &[
                r"C:\Program Files\nodejs\node.exe",
                r"C:\workspace\cursor-agent\versions\test\index.js",
            ],
        )],
    };
    assert_eq!(identify_agent_in_job(&lookalike), None);
}

#[test]
fn identify_agent_in_job_prefers_recognized_process_group_leader() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 42,
        processes: vec![
            foreground_process(42, "claude", &["claude"]),
            foreground_process(43, "node", &["node", "/tmp/mcp/bin/codex"]),
        ],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Claude, "claude".to_string()))
    );
}

#[test]
fn identify_agent_in_job_falls_back_when_process_group_leader_is_unrecognized() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 42,
        processes: vec![
            foreground_process(42, "bash", &["bash"]),
            foreground_process(43, "node", &["node", "/tmp/mcp/bin/codex"]),
        ],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Codex, "codex".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_python_version_wrapped_hermes() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "python3.12",
            &[
                "/nix/store/example/bin/python3.12",
                "/nix/store/example/bin/hermes",
                "--resume",
                "session-id",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Hermes, "hermes".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_nix_wrapped_codex_from_cmdline_argv0() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            ".codex-wrapped",
            &["/etc/profiles/per-user/user/bin/codex", "--model", "gpt-5"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Codex, "codex".to_string()))
    );
}

#[test]
fn identify_agent_in_job_canonicalizes_nix_wrapped_aliases_from_cmdline_argv0() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            ".claude-code-wrapped",
            &["/nix/store/example/bin/claude-code"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Claude, "claude".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_shell_wrapped_pi() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "sh",
            &["/bin/sh", "/tmp/test-bin/pi"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Pi, "pi".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_bun_wrapped_omp() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "bun",
            &["bun", "/home/can/.bun/bin/omp"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Omp, "omp".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_node_wrapped_pi_package_cli() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "node.exe",
            &[
                "node.exe",
                "C:\\Users\\herdr\\AppData\\Roaming\\npm\\node_modules\\@earendil-works\\pi-coding-agent\\dist\\cli.js",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Pi, "pi".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_node_wrapped_pi_bundled_cli() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "node.exe",
            &[
                r"C:\Users\herdr\AppData\Local\pi-node\current\node.exe",
                r"C:\Users\herdr\AppData\Local\pi-node\current/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Pi, "pi".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_node_wrapped_mastracode_package_cli() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "node.exe",
            &[
                "node.exe",
                "C:\\Users\\herdr\\AppData\\Roaming\\npm\\node_modules\\mastracode\\dist\\cli.js",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Mastracode, "mastracode".to_string()))
    );
}

#[test]
fn identify_agent_in_job_ignores_non_cli_pi_package_scripts() {
    for script in [
        r"C:\Users\herdr\AppData\Roaming\npm\node_modules\@earendil-works\pi-coding-agent\scripts\build.js",
        r"C:\Users\herdr\AppData\Local\pi-node\current\node_modules\@earendil-works\pi-coding-agent\dist\bundle\update.js",
        r"C:\workspace\dist\bundle\cli.js",
        r"C:\workspace\node_modules\other-package\dist\bundle\cli.js",
        r"C:\workspace\node_modules\@earendil-works\pi-coding-agent\dist\cli.exe",
        r"C:\workspace\node_modules\@earendil-works\pi-coding-agent\dist\cli.js\other.js",
        r"C:\workspace\node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.exe",
        r"C:\workspace\node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.js\other.js",
    ] {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(123, "node.exe", &["node.exe", script])],
        };

        assert_eq!(identify_agent_in_job(&job), None, "script: {script}");
    }
}

#[test]
fn identify_agent_in_job_detects_windows_cmd_wrapped_codex() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "cmd.exe",
            &[
                "cmd.exe",
                "/D",
                "/S",
                "/C",
                "C:\\Users\\herdr\\AppData\\Roaming\\npm\\codex.cmd --model gpt-5",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Codex, "codex".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_powershell_file_wrapped_claude() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "powershell.exe",
            &[
                "powershell.exe",
                "-NoProfile",
                "-File",
                "C:\\Users\\herdr\\Documents\\PowerShell\\Scripts\\claude.ps1",
            ],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Claude, "claude".to_string()))
    );
}

// A plain shell pane launched with herdr's injected prompt integration
// must still classify as a shell, not an agent, even though its argv now
// carries a -Command payload.
#[test]
fn identify_agent_in_job_ignores_herdr_powershell_shell_integration_argv() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "powershell.exe",
            &[
                "powershell.exe",
                "-NoExit",
                "-Command",
                r"if ($null -eq $global:__HerdrOriginalPrompt) { $global:__HerdrOriginalPrompt = $function:prompt; function global:prompt { $out = @(& $global:__HerdrOriginalPrompt) -join ' '; $loc = $ExecutionContext.SessionState.Path.CurrentLocation; if ($loc.Provider.Name -eq 'FileSystem') { try { [Environment]::CurrentDirectory = $loc.ProviderPath } catch {}; $esc = [string][char]27; $out += $esc + ']9;9;' + $loc.ProviderPath + $esc + '\' }; $out } }",
            ],
        )],
    };

    assert_eq!(identify_agent_in_job(&job), None);
}

#[test]
fn identify_agent_in_job_detects_opencode2_as_opencode() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "opencode2",
            &["opencode2", "--standalone"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::OpenCode, "opencode2".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_opencode_exe_from_pnpm_package() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "opencode.exe",
            &["/home/user/.local/share/pnpm/global/node_modules/opencode-ai/bin/opencode.exe"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::OpenCode, "opencode.exe".to_string()))
    );
}

#[test]
fn identify_agent_in_job_detects_opencode_exe_from_argv0_path() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            123,
            "MainThread",
            &["/home/user/.local/share/pnpm/global/node_modules/opencode-ai/bin/opencode.exe"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::OpenCode, "opencode".to_string()))
    );
}

#[test]
fn wrapped_agent_name_from_runtime_argv_ignores_plain_shell_flags() {
    assert_eq!(
        wrapped_agent_name_from_runtime_argv("bash", Some(&["bash".into(), "-lc".into()])),
        None
    );
}

#[test]
fn identify_agent_in_job_ignores_python_c_argument_named_codex() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "python3",
            &["python3", "-c", "import time; time.sleep(60)", "/tmp/codex"],
        )],
    };

    assert_eq!(identify_agent_in_job(&job), None);
}

#[test]
fn identify_agent_in_job_ignores_node_eval_argument_named_codex() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "node",
            &["node", "-e", "setTimeout(() => {}, 60000)", "/tmp/codex"],
        )],
    };

    assert_eq!(identify_agent_in_job(&job), None);
}

#[test]
fn identify_agent_in_job_ignores_shell_c_argument_named_codex() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "bash",
            &["bash", "-c", "sleep 60", "/tmp/codex"],
        )],
    };

    assert_eq!(identify_agent_in_job(&job), None);
}

#[test]
fn identify_agent_in_job_detects_python_script_named_codex() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 123,
        processes: vec![foreground_process(
            1,
            "python3",
            &["python3", "/tmp/codex", "--model", "gpt-5"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Codex, "codex".to_string()))
    );
}

#[test]
fn cmdline_argv0_agent_name_canonicalizes_known_aliases() {
    assert_eq!(
        cmdline_argv0_agent_name("/nix/store/example/bin/ghcs"),
        Some("copilot".to_string())
    );
}

#[test]
fn cmdline_argv0_agent_name_requires_exact_agent_basename() {
    assert_eq!(cmdline_argv0_agent_name("/tmp/my-codex-helper"), None);
}

#[cfg(unix)]
#[test]
fn identify_agent_in_job_resolves_cursor_agent_symlink_argv0() {
    let dir = temp_detection_path("cursor-agent-symlink");
    std::fs::create_dir_all(&dir).expect("test directory should be created");
    let target = dir.join("cursor-agent");
    let link = dir.join("agent");
    std::fs::write(&target, b"#!/bin/sh\n").expect("target should be written");
    std::os::unix::fs::symlink(&target, &link).expect("symlink should be created");

    let argv0 = link.to_string_lossy().into_owned();
    let job = crate::platform::ForegroundJob {
        process_group_id: 42,
        processes: vec![foreground_process(
            42,
            "MainThread",
            &[&argv0, "--use-system-ca", "/tmp/index.js"],
        )],
    };

    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Cursor, "cursor".to_string()))
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---- Screen detection routing ----

// ---- Process identification (real PTY) ----

#[cfg(target_os = "linux")]
fn open_test_pty() -> portable_pty::PtyPair {
    portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open pty")
}

#[cfg(target_os = "linux")]
#[test]
fn foreground_job_detects_sleep() {
    use portable_pty::CommandBuilder;

    let pair = open_test_pty();

    // Spawn "sleep 999" — a known, deterministic process
    let mut cmd = CommandBuilder::new("sleep");
    cmd.arg("999");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    let pid = child.process_id().expect("no pid");

    // Give the process a moment to become the foreground group
    std::thread::sleep(std::time::Duration::from_millis(50));

    let job = foreground_job(pid).expect("expected foreground job");
    assert!(
        job.processes.iter().any(|p| p.name == "sleep"),
        "expected sleep in {job:?}"
    );
    assert_eq!(
        identify_agent_in_job(&job),
        None,
        "sleep should not map to an agent"
    );

    // Clean up
    child.kill().ok();
    child.wait().ok();
}

#[cfg(target_os = "linux")]
#[test]
fn foreground_job_detects_shell_running_command() {
    use portable_pty::CommandBuilder;
    use std::io::Write;

    let pair = open_test_pty();

    // Spawn a shell, then run a command inside it
    let cmd = CommandBuilder::new("sh");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    let pid = child.process_id().expect("no pid");

    // Write a command to the shell
    let mut writer = pair.master.take_writer().expect("no writer");
    // Use exec so sleep replaces sh as the foreground process
    writer.write_all(b"exec sleep 999\n").ok();
    drop(writer);

    std::thread::sleep(std::time::Duration::from_millis(100));

    let job = foreground_job(pid).expect("expected foreground job");
    assert!(
        job.processes.iter().any(|p| p.name == "sleep"),
        "expected sleep in {job:?}"
    );
    assert_eq!(
        identify_agent_in_job(&job),
        None,
        "sleep should not map to an agent"
    );

    child.kill().ok();
    child.wait().ok();
}

#[cfg(target_os = "linux")]
#[test]
fn foreground_job_detects_agent_behind_shell_wrapper() {
    use portable_pty::CommandBuilder;

    let pair = open_test_pty();

    let mut cmd = CommandBuilder::new("bash");
    cmd.arg("-c");
    cmd.arg("bash -c 'exec -a codex sleep 999' & wait");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    let pid = child.process_id().expect("no pid");
    std::thread::sleep(std::time::Duration::from_millis(100));

    let job = foreground_job(pid);
    let process_group_id = job.as_ref().map(|job| job.process_group_id).unwrap_or(pid);
    unsafe {
        libc::kill(-(process_group_id as i32), libc::SIGKILL);
    }
    child.wait().ok();

    let job = job.expect("expected foreground job");
    assert!(
        job.processes.iter().any(|process| process.name == "bash")
            && job.processes.iter().any(|process| {
                process.name == "sleep"
                    && process
                        .argv
                        .as_deref()
                        .and_then(|argv| argv.first())
                        .is_some_and(|argv0| argv0 == "codex")
            }),
        "expected wrapper and agent child in {job:?}"
    );
    assert_eq!(
        identify_agent_in_job(&job),
        Some((AgentKind::Codex, "codex".to_string()))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn proc_stat_parsing_handles_spaces_in_comm() {
    // Verify our /proc/pid/stat parser correctly extracts fields
    // even when (comm) could contain spaces.
    let pid = std::process::id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();

    // Our parsing: find last ')' then split the rest
    let close_paren = stat.rfind(')').expect("should have closing paren");
    let rest = &stat[close_paren + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();

    // We should have enough fields (at least 6 for tpgid)
    assert!(
        fields.len() >= 6,
        "not enough fields in stat: {}",
        fields.len()
    );

    // Field 0 should be a valid state char (S, R, D, etc.)
    let state = fields[0];
    assert!(
        ["S", "R", "D", "Z", "T", "t", "W", "X", "I"].contains(&state),
        "unexpected state: {state}"
    );

    // Field 5 (tpgid) should parse as i32 (can be -1 if no controlling terminal)
    let tpgid: i32 = fields[5].parse().expect("tpgid should be a number");
    // In CI/test environments without a terminal, tpgid is typically -1
    let _ = tpgid;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn parse_agent_env_hint_accepts_known_agents() {
    assert_eq!(
        super::identify::parse_agent_env_hint(b"PATH=/bin\0HERDR_AGENT=claude\0TERM=xterm\0"),
        Some(AgentKind::Claude)
    );
    assert_eq!(
        super::identify::parse_agent_env_hint(b"HERDR_AGENT=codex"),
        Some(AgentKind::Codex)
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn parse_agent_env_hint_ignores_missing_or_unknown_agents() {
    assert_eq!(
        super::identify::parse_agent_env_hint(b"PATH=/bin\0TERM=xterm\0"),
        None
    );
    assert_eq!(
        super::identify::parse_agent_env_hint(b"HERDR_AGENT=not-an-agent\0"),
        None
    );
}
