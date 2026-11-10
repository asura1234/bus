use super::*;
use std::io;
use std::path::Path;

#[cfg(unix)]
#[test]
fn pane_launch_env_keeps_yolo_private_to_bus() {
    for override_marker in [false, true] {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args([
            "-c",
            "test -z \"${BUS_YOLO+x}\" && test \"$BUS_LAUNCH_ID\" = own-launch",
        ]);
        cmd.env("BUS_YOLO", "1");
        let mut extra = vec![("BUS_LAUNCH_ID".into(), "own-launch".into())];
        if override_marker {
            extra.push(("BUS_YOLO".into(), "1".into()));
        }
        apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::from_extra(extra)).unwrap();
        let mut spawned =
            crate::terminal::runtime::spawn::spawn_with_portable_pty(24, 80, cmd).unwrap();
        assert!(
            spawned.child.wait().unwrap().success(),
            "private mode leaked into pane"
        );
    }
}

#[test]
fn pane_launch_env_removes_outer_codex_thread_id() {
    let mut cmd = CommandBuilder::new("shell");
    cmd.env("CODEX_THREAD_ID", "outer-session");

    apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::default()).unwrap();

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
        apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::from_extra(extra)).unwrap();

        for (key, _) in session {
            assert!(
                cmd.get_env(key).is_none(),
                "inherited session marker: {key}"
            );
        }
        for (key, value) in config {
            if key == "PATH" {
                let paths = std::env::split_paths(cmd.get_env(key).unwrap()).collect::<Vec<_>>();
                assert_eq!(
                    &paths[1..],
                    std::env::split_paths(value).collect::<Vec<_>>()
                );
            } else {
                assert_eq!(cmd.get_env(key), Some(std::ffi::OsStr::new(value)), "{key}");
            }
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

#[cfg(unix)]
#[test]
fn pane_shell_without_env_uses_account_login_shell() {
    // Independent account lookup: the builder ignores the inherited SHELL.
    let mut account = CommandBuilder::new_default_prog();
    account.env_remove("SHELL");
    let account_shell = account.get_shell();
    for shell_env in [None, Some("  ".to_owned())] {
        assert_eq!(pane_shell_from("", shell_env), account_shell);
    }
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
            "/__bus_missing_shell__",
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
    let base = crate::utils::test_temp::unique_temp_path("bus-login-shell-path");
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shell = bin.join("fake-shell");
    std::fs::write(&shell, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _path = crate::utils::env::PathOverride::set(&bin);

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
    drop(_path);
    let _ = std::fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn login_shell_from_relative_path_runs_in_the_pane_directory() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = self::env::env_lock();
    let base = crate::utils::test_temp::unique_temp_path("bus-relative-login-shell");
    let bin = base.join("bin");
    let pane_cwd = base.join("pane");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&pane_cwd).unwrap();
    let shell = bin.join("fake-shell");
    std::fs::write(&shell, "#!/bin/sh\nexit 17\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut relative_bin = std::path::PathBuf::new();
    for component in std::env::current_dir().unwrap().components() {
        if matches!(component, std::path::Component::Normal(_)) {
            relative_bin.push("..");
        }
    }
    relative_bin.push(bin.strip_prefix("/").unwrap());
    let _path = crate::utils::env::PathOverride::set(&relative_bin);

    let spawn_exit = |configured_shell: &str| {
        let mut cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new(
                configured_shell,
                crate::utils::config::ShellModeConfig::Login,
            ),
            ShellLaunchTarget::OtherUnix,
        )?;
        cmd.cwd(&pane_cwd);
        let mut spawned = crate::terminal::runtime::spawn::spawn_with_portable_pty(24, 80, cmd)?;
        spawned.child.wait()
    };
    let absolute_result = spawn_exit(shell.to_str().unwrap());
    let result = spawn_exit("fake-shell");

    drop(_path);
    std::fs::remove_dir_all(&base).unwrap();

    assert_eq!(
        absolute_result.unwrap().exit_code(),
        17,
        "the fixture shell must run successfully from the pane cwd"
    );
    assert!(
        result.is_ok(),
        "a login shell found on PATH must launch after changing to the pane cwd: {result:?}"
    );
    assert_eq!(result.unwrap().exit_code(), 17);
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

#[test]
fn native_pane_defaults_disable_claude_updates_and_keep_explicit_settings() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("DISABLE_AUTOUPDATER");
    std::env::remove_var("DISABLE_AUTOUPDATER");
    let mut cmd = CommandBuilder::new("shell");
    apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::default()).unwrap();
    let default = cmd.get_env("DISABLE_AUTOUPDATER").map(|v| v.to_owned());
    std::env::set_var("DISABLE_AUTOUPDATER", "0");
    let mut inherited = CommandBuilder::new("shell");
    apply_pane_launch_env(&mut inherited, &PaneLaunchEnv::default()).unwrap();
    let inherited = inherited
        .get_env("DISABLE_AUTOUPDATER")
        .map(|v| v.to_owned());
    let mut explicit = CommandBuilder::new("shell");
    apply_pane_launch_env(
        &mut explicit,
        &PaneLaunchEnv::from_extra(vec![("DISABLE_AUTOUPDATER".into(), "user".into())]),
    )
    .unwrap();
    let explicit = explicit
        .get_env("DISABLE_AUTOUPDATER")
        .map(|v| v.to_owned());
    match old {
        Some(v) => std::env::set_var("DISABLE_AUTOUPDATER", v),
        None => std::env::remove_var("DISABLE_AUTOUPDATER"),
    }
    assert_eq!(default.as_deref(), Some(std::ffi::OsStr::new("1")));
    assert_eq!(inherited.as_deref(), Some(std::ffi::OsStr::new("0")));
    assert_eq!(explicit.as_deref(), Some(std::ffi::OsStr::new("user")));
}

#[test]
fn pane_launch_env_adds_private_bus_directory_to_effective_path() {
    let executable = std::env::current_exe().unwrap();
    let directory = crate::utils::paths::data_dir().join("cli-bin");
    let inherited = std::env::join_paths([Path::new("/inherited/bin")]).unwrap();
    let override_path =
        std::env::join_paths([Path::new("/provider/bin"), Path::new("/user/bin")]).unwrap();
    let process_path = std::env::var_os("PATH");
    for explicit in [false, true] {
        let mut cmd = CommandBuilder::new("shell");
        cmd.env_clear();
        cmd.env("PATH", &inherited);
        cmd.env("HERDR_BIN_PATH", "/stale/bus");
        let extras = if explicit {
            vec![("PATH".into(), override_path.to_str().unwrap().into())]
        } else {
            Vec::new()
        };
        apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::from_extra(extras)).unwrap();
        let paths = std::env::split_paths(cmd.get_env("PATH").unwrap()).collect::<Vec<_>>();
        assert_eq!(
            paths.first().map(|p| p.as_path()),
            Some(directory.as_path()),
            "the running Bus CLI must be reachable before stale installs"
        );
        let original = if explicit { &override_path } else { &inherited };
        assert_eq!(
            &paths[1..],
            std::env::split_paths(original).collect::<Vec<_>>()
        );
        assert_eq!(cmd.get_env("HERDR_BIN_PATH"), Some(executable.as_os_str()));
    }
    assert_eq!(
        std::env::var_os("PATH"),
        process_path,
        "pane setup must not mutate the server environment"
    );
}

#[test]
fn bus_cli_path_keeps_missing_empty_and_existing_entries() {
    let directory = crate::utils::test_temp::unique_temp_path("Bus path with spaces");
    let executable = directory.join("bus");
    let data = directory.join("session");
    let alias = data.join("cli-bin");
    for original in [
        None,
        Some(std::ffi::OsString::new()),
        Some(
            std::env::join_paths([
                Path::new("/provider/bin"),
                directory.as_path(),
                alias.as_path(),
                Path::new("/user/bin"),
            ])
            .unwrap(),
        ),
    ] {
        let mut cmd = CommandBuilder::new("shell");
        cmd.env_clear();
        if let Some(path) = &original {
            cmd.env("PATH", path);
        }
        crate::terminal::runtime::cli_env::apply_bus_cli_env(&mut cmd, &executable, &data).unwrap();
        let paths = std::env::split_paths(cmd.get_env("PATH").unwrap()).collect::<Vec<_>>();
        assert_eq!(paths[0], alias);
        assert_eq!(paths.iter().filter(|path| *path == &alias).count(), 1);
        let retained = original
            .iter()
            .flat_map(std::env::split_paths)
            .filter(|path| path != &alias)
            .collect::<Vec<_>>();
        assert_eq!(&paths[1..], retained);
    }
}

#[cfg(unix)]
#[test]
fn bus_cli_path_resolves_dev_and_installed_binaries_ahead_of_stale_bus() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::utils::test_temp::unique_temp_path("bus-command-path");
    let stale = root.join("stale");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("bus"), "#!/bin/sh\nexit 42\n").unwrap();
    std::fs::set_permissions(stale.join("bus"), std::fs::Permissions::from_mode(0o755)).unwrap();
    for location in ["checkout/target/debug", "installed app/bin"] {
        let directory = root.join(location);
        let executable = directory.join("bus");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "bus"]);
        cmd.env_clear();
        cmd.env("PATH", &stale);
        let data = root.join("session");
        crate::terminal::runtime::prepare_bus_cli(&data, &executable).unwrap();
        crate::terminal::runtime::cli_env::apply_bus_cli_env(&mut cmd, &executable, &data).unwrap();
        let mut spawned =
            crate::terminal::runtime::spawn::spawn_with_portable_pty(24, 80, cmd).unwrap();
        assert!(
            spawned.child.wait().unwrap().success(),
            "{location}: bare bus selected the stale CLI"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn bus_cli_path_preserves_non_utf8_directories() {
    use std::os::unix::ffi::OsStringExt;
    let directory = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/bus-\xff".to_vec()));
    let inherited =
        std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/provider-\xfe".to_vec()));
    let mut cmd = CommandBuilder::new("shell");
    cmd.env_clear();
    cmd.env("PATH", std::env::join_paths([&inherited]).unwrap());
    let data = directory.join("session");
    crate::terminal::runtime::cli_env::apply_bus_cli_env(&mut cmd, &directory.join("bus"), &data)
        .unwrap();
    assert_eq!(
        std::env::split_paths(cmd.get_env("PATH").unwrap()).collect::<Vec<_>>(),
        [data.join("cli-bin"), inherited]
    );
}

#[cfg(unix)]
#[test]
fn bus_cli_alias_preserves_provider_resolution_and_refreshes_the_running_binary() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::utils::test_temp::unique_temp_path("bus-shared-bin-priority");
    let shared = root.join("shared bin");
    let selected = root.join("provider bin");
    let data = root.join("session");
    for (directory, name, code) in [
        (&shared, "bus", 0),
        (&shared, "claude", 7),
        (&selected, "claude", 0),
    ] {
        std::fs::create_dir_all(directory).unwrap();
        let file = directory.join(name);
        std::fs::write(&file, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let executable = shared.join("bus");
    crate::terminal::runtime::prepare_bus_cli(&data, &executable).unwrap();
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args(["-c", "claude && bus"]);
    cmd.env_clear();
    cmd.env("PATH", std::env::join_paths([&selected, &shared]).unwrap());
    crate::terminal::runtime::cli_env::apply_bus_cli_env(&mut cmd, &executable, &data).unwrap();
    let mut spawned =
        crate::terminal::runtime::spawn::spawn_with_portable_pty(24, 80, cmd).unwrap();
    assert!(
        spawned.child.wait().unwrap().success(),
        "the selected provider must win over a same-named executable beside Bus"
    );
    assert_eq!(std::fs::read_dir(data.join("cli-bin")).unwrap().count(), 1);
    assert_eq!(
        std::fs::metadata(data.join("cli-bin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    // The next server start may use another executable. Existing PATH values
    // keep resolving the stable alias, now pointing at that server's image.
    let moved = root.join("another Bus");
    std::fs::write(&moved, "#!/bin/sh\nexit 17\n").unwrap();
    std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::terminal::runtime::prepare_bus_cli(&data, &moved).unwrap();
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args(["-c", "bus"]);
    cmd.env_clear();
    cmd.env("PATH", data.join("cli-bin"));
    let mut spawned =
        crate::terminal::runtime::spawn::spawn_with_portable_pty(24, 80, cmd).unwrap();
    assert_eq!(spawned.child.wait().unwrap().exit_code(), 17);
    assert_eq!(std::fs::read_link(data.join("cli-bin/bus")).unwrap(), moved);
    std::fs::remove_dir_all(root).unwrap();
}
