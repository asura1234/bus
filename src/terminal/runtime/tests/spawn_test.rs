use super::*;
use std::io;

#[test]
fn pane_launch_env_removes_outer_codex_thread_id() {
    let mut cmd = CommandBuilder::new("shell");
    cmd.env("CODEX_THREAD_ID", "outer-session");

    apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::default());

    assert!(cmd.get_env("CODEX_THREAD_ID").is_none());
}

#[test]
fn pane_launch_env_isolates_agent_sessions_without_losing_configuration() {
    // Session context exported by Claude's Bash tool and Codex's exec tool.
    let session = [
        ("CLAUDECODE", "1"),
        ("CLAUDE_CODE_CHILD_SESSION", "1"),
        ("CLAUDE_CODE_ENTRYPOINT", "cli"),
        ("CLAUDE_CODE_SESSION_ID", "outer-claude"),
        ("CLAUDE_CODE_SESSION_ATTENDED", "0"),
        ("CLAUDE_CODE_SSE_PORT", "12345"),
        ("CLAUDE_CODE_MESSAGING_SOCKET", "/outer/message.sock"),
        ("CLAUDE_CODE_MESSAGING_TOKEN", "outer-message-token"),
        ("CLAUDE_CODE_SANDBOXED", "1"),
        ("CLAUDE_PID", "123"),
        ("CLAUDE_JOB_DIR", "/outer/job"),
        ("CODEX_THREAD_ID", "outer-thread"),
        ("CODEX_SESSION_ID", "outer-codex"),
        ("CODEX_SANDBOX", "seatbelt"),
        ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
        ("CODEX_PERMISSION_PROFILE", ":workspace"),
        ("CODEX_ESCALATE_SOCKET", "/outer/escalate.sock"),
        ("CODEX_EXEC_SERVER_NOISE_AUTH_TOKEN", "outer-exec-token"),
    ];
    let config = [
        ("CLAUDE_CONFIG_DIR", "/user/claude"),
        ("CLAUDE_CODE_EXECPATH", "/user/bin/claude"),
        ("CLAUDE_CODE_SHELL", "/bin/zsh"),
        ("CLAUDE_CODE_SUBAGENT_MODEL", "haiku"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("CLAUDE_EFFORT", "medium"),
        ("ANTHROPIC_API_KEY", "user-api-key"),
        ("ANTHROPIC_BASE_URL", "https://provider.example"),
        ("CODEX_HOME", "/user/codex"),
        ("CODEX_API_KEY", "user-codex-key"),
        ("CODEX_MANAGED_BY_NPM", "1"),
        ("CODEX_CI", "1"),
        ("OPENAI_API_KEY", "user-openai-key"),
        ("HTTP_PROXY", "http://proxy.example"),
        ("PATH", "/user/bin"),
    ];
    // Both a new shell and a resumed agent reach the same pane-env boundary.
    // Explicit launch env must not put a parent's session context back.
    for restore_from_launch_env in [false, true] {
        let mut cmd = CommandBuilder::new("shell");
        cmd.env_clear();
        for (key, value) in session.into_iter().chain(config) {
            cmd.env(key, value);
        }
        let mut extra = vec![
            ("BUS_LAUNCH_ID".into(), "own-launch".into()),
            ("BUS_CALLBACK_DIR".into(), "/own/callbacks".into()),
        ];
        if restore_from_launch_env {
            extra.extend(session.map(|(key, value)| (key.into(), value.into())));
        }
        apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::from_extra(extra));

        for (key, _) in session {
            assert!(
                cmd.get_env(key).is_none(),
                "inherited session marker: {key}"
            );
        }
        for (key, value) in config {
            assert_eq!(cmd.get_env(key), Some(std::ffi::OsStr::new(value)), "{key}");
        }
        for (key, value) in [
            ("BUS_LAUNCH_ID", "own-launch"),
            ("BUS_CALLBACK_DIR", "/own/callbacks"),
        ] {
            assert_eq!(cmd.get_env(key), Some(std::ffi::OsStr::new(value)), "{key}");
        }
    }
}

#[test]
fn pane_terminal_identity_removes_outer_windows_terminal_session() {
    let mut cmd = CommandBuilder::new("shell");
    cmd.env("WT_SESSION", "outer-session");

    apply_pane_terminal_env(&mut cmd);

    assert!(cmd.get_env("WT_SESSION").is_none());
}

#[test]
fn pane_shell_prefers_configured_shell() {
    assert_eq!(
        pane_shell_from("/usr/bin/nu", Some("/bin/bash".to_string())),
        "/usr/bin/nu"
    );
}

#[cfg(not(windows))]
#[test]
fn pane_shell_falls_back_to_shell_env() {
    assert_eq!(
        pane_shell_from("", Some("/bin/bash".to_string())),
        "/bin/bash"
    );
}

#[cfg(windows)]
#[test]
fn pane_shell_ignores_shell_env_on_windows() {
    assert_eq!(
        pane_shell_from("", Some("c:\\windows\\system32\\cmd.exe".to_string())),
        default_pane_shell()
    );
}

#[test]
fn pane_shell_ignores_empty_values() {
    assert_eq!(
        pane_shell_from("   ", Some("  ".to_string())),
        default_pane_shell()
    );
    assert_eq!(pane_shell_from("", None), default_pane_shell());
}

#[test]
fn shell_mode_auto_uses_login_shell_only_on_macos() {
    assert!(shell_mode_uses_login_shell(
        crate::utils::config::ShellModeConfig::Auto,
        ShellLaunchTarget::Macos
    ));
    assert!(!shell_mode_uses_login_shell(
        crate::utils::config::ShellModeConfig::Auto,
        ShellLaunchTarget::OtherUnix
    ));
    assert!(!shell_mode_uses_login_shell(
        crate::utils::config::ShellModeConfig::Auto,
        ShellLaunchTarget::Windows
    ));
    assert!(shell_mode_uses_login_shell(
        crate::utils::config::ShellModeConfig::Login,
        ShellLaunchTarget::OtherUnix
    ));
    assert!(!shell_mode_uses_login_shell(
        crate::utils::config::ShellModeConfig::NonLogin,
        ShellLaunchTarget::Macos
    ));
}

#[cfg(unix)]
#[test]
fn login_shell_builder_uses_default_prog_with_resolved_shell_env() {
    let cmd = pane_shell_command_builder_for_target(
        PaneShellConfig::new("/bin/sh", crate::utils::config::ShellModeConfig::Login),
        ShellLaunchTarget::OtherUnix,
    )
    .unwrap();
    assert!(cmd.is_default_prog());
    assert_eq!(
        cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
        Some("/bin/sh")
    );
}

#[cfg(unix)]
#[test]
fn auto_shell_builder_uses_login_shell_on_macos_target() {
    let cmd = pane_shell_command_builder_for_target(
        PaneShellConfig::new("/bin/sh", crate::utils::config::ShellModeConfig::Auto),
        ShellLaunchTarget::Macos,
    )
    .unwrap();
    assert!(cmd.is_default_prog());
    assert_eq!(
        cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
        Some("/bin/sh")
    );
}

#[test]
fn auto_shell_builder_keeps_direct_shell_on_non_macos_target() {
    let cmd = pane_shell_command_builder_for_target(
        PaneShellConfig::new("/bin/sh", crate::utils::config::ShellModeConfig::Auto),
        ShellLaunchTarget::OtherUnix,
    )
    .unwrap();
    assert!(!cmd.is_default_prog());
    assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("/bin/sh")]);
}

#[test]
fn windows_powershell_builder_injects_prompt_cwd_shell_integration() {
    for shell in [
        "powershell.exe",
        "pwsh.exe",
        "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
    ] {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new(shell, crate::utils::config::ShellModeConfig::NonLogin),
            ShellLaunchTarget::Windows,
        )
        .unwrap();

        assert_eq!(
            cmd.get_argv(),
            &[
                std::ffi::OsString::from(shell),
                std::ffi::OsString::from("-NoExit"),
                std::ffi::OsString::from("-Command"),
                std::ffi::OsString::from(WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND),
            ]
        );
    }

    let script = WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND;
    let cwd_sync = script
        .find("[Environment]::CurrentDirectory = $loc.ProviderPath")
        .expect("wrapper must synchronize the Win32 process cwd");
    let osc_report = script.find("]9;9;").expect("wrapper must emit OSC 9;9");
    assert!(cwd_sync < osc_report, "cwd sync must precede OSC report");
    assert!(
        script.contains("$global:__HerdrOriginalPrompt = $function:prompt"),
        "must wrap the profile-defined prompt: {script}"
    );
    assert!(
        script.contains("$null -eq $global:__HerdrOriginalPrompt"),
        "wrap must be idempotent for nested sessions: {script}"
    );
    assert!(
        script.contains("'FileSystem'"),
        "must not report non-filesystem provider paths: {script}"
    );
    assert!(
        !script.contains('"'),
        "double quotes corrupt the powershell.exe command-line round-trip: {script}"
    );
    let invoke_original = script
        .find("@(& $global:__HerdrOriginalPrompt)")
        .expect("wrapper must invoke the original prompt");
    let cwd_lookup = script
        .find("$loc =")
        .expect("wrapper must look up the current location");
    assert!(
            invoke_original < cwd_lookup,
            "original prompt must run first or $? is reset before a status-aware prompt reads it: {script}"
        );
}

#[test]
fn windows_non_powershell_builder_launches_plain_shell() {
    let cmd = pane_shell_command_builder_for_target(
        PaneShellConfig::new("cmd.exe", crate::utils::config::ShellModeConfig::NonLogin),
        ShellLaunchTarget::Windows,
    )
    .unwrap();

    assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("cmd.exe")]);
}

#[test]
fn unix_powershell_builder_launches_plain_shell() {
    let cmd = pane_shell_command_builder_for_target(
        PaneShellConfig::new("pwsh", crate::utils::config::ShellModeConfig::NonLogin),
        ShellLaunchTarget::OtherUnix,
    )
    .unwrap();

    assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("pwsh")]);
}

#[test]
fn windows_powershell_pane_shell_predicate_requires_windows_and_non_login() {
    let pwsh = PaneShellConfig::new("pwsh.exe", crate::utils::config::ShellModeConfig::NonLogin);
    assert!(uses_windows_powershell_pane_shell_for_target(
        pwsh,
        ShellLaunchTarget::Windows
    ));
    assert!(!uses_windows_powershell_pane_shell_for_target(
        pwsh,
        ShellLaunchTarget::OtherUnix
    ));
    assert!(!uses_windows_powershell_pane_shell_for_target(
        pwsh,
        ShellLaunchTarget::Macos
    ));
    assert!(!uses_windows_powershell_pane_shell_for_target(
        PaneShellConfig::new("pwsh.exe", crate::utils::config::ShellModeConfig::Login),
        ShellLaunchTarget::Windows
    ));
    assert!(!uses_windows_powershell_pane_shell_for_target(
        PaneShellConfig::new("cmd.exe", crate::utils::config::ShellModeConfig::NonLogin),
        ShellLaunchTarget::Windows
    ));
}

#[test]
fn login_shell_builder_rejects_missing_shell_instead_of_falling_back() {
    let err = pane_shell_command_builder_for_target(
        PaneShellConfig::new(
            "/__herdr_missing_shell__",
            crate::utils::config::ShellModeConfig::Login,
        ),
        ShellLaunchTarget::OtherUnix,
    )
    .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn login_shell_builder_resolves_bare_shell_names_from_path() {
    let _lock = self::env::env_lock();
    let base = std::env::temp_dir().join(format!(
        "herdr-login-shell-path-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shell = bin.join("fake-shell");
    std::fs::write(&shell, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let original_path = std::env::var_os("PATH");
    std::env::set_var("PATH", &bin);

    let cmd = pane_shell_command_builder_for_target(
        PaneShellConfig::new("fake-shell", crate::utils::config::ShellModeConfig::Login),
        ShellLaunchTarget::OtherUnix,
    )
    .unwrap();

    assert!(cmd.is_default_prog());
    assert_eq!(
        cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
        shell.to_str()
    );
    match original_path {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    let _ = std::fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn login_shell_resolution_preserves_shell_paths() {
    assert_eq!(resolve_shell_for_login_mode("/bin/sh").unwrap(), "/bin/sh");
}

#[test]
fn non_login_shell_builder_execs_resolved_shell_directly() {
    let cmd = pane_shell_command_builder(PaneShellConfig::new(
        "/bin/sh",
        crate::utils::config::ShellModeConfig::NonLogin,
    ))
    .unwrap();
    assert!(!cmd.is_default_prog());
    assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("/bin/sh")]);
}

#[cfg(unix)]
#[test]
fn pane_terminal_identity_overrides_outer_terminal_env() {
    let output = capture_shell_output("printf '%s\\n%s\\n' \"$TERM\" \"$COLORTERM\"", &[]);
    assert_eq!(output, "xterm-256color\ntruecolor\n");
}

#[cfg(unix)]
#[test]
fn pane_terminal_identity_allows_explicit_override() {
    let output = capture_shell_output(
        "printf '%s\\n%s\\n' \"$TERM\" \"$COLORTERM\"",
        &[("TERM", "vt100"), ("COLORTERM", "24bit")],
    );
    assert_eq!(output, "vt100\n24bit\n");
}

#[cfg(unix)]
#[test]
fn pane_terminal_does_not_inherit_outer_no_color() {
    let output = capture_shell_output("printf '%s' \"${NO_COLOR-unset}\"", &[]);
    assert_eq!(output, "unset");
}

#[cfg(unix)]
#[test]
fn pane_terminal_allows_explicit_no_color_override() {
    let output = capture_shell_output("printf '%s' \"${NO_COLOR-unset}\"", &[("NO_COLOR", "1")]);
    assert_eq!(output, "1");
}
