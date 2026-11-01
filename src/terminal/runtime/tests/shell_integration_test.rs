use super::*;

#[test]
fn unknown_process_cannot_authorize_even_a_new_prompt_mark() {
    let mut command = CommandBuilder::new("/bin/bash");
    let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
        "BUS_LAUNCH_ID".into(),
        "unknown-process".into(),
    )]);
    let Some(mut integration) =
        ShellIntegration::install_for_agent_launch(&mut command, &launch).unwrap()
    else {
        return;
    };
    integration.start_wait(None);
    assert!(integration.waiting_for_prompt(None, 1));
    assert!(integration.waiting_for_prompt(Some(u32::MAX), 2));
}

#[test]
fn fork_image_cannot_authorize_even_a_launch_specific_prompt_mark() {
    let mut command = CommandBuilder::new("/bin/bash");
    let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
        "BUS_LAUNCH_ID".into(),
        "fork-image".into(),
    )]);
    let Some(mut integration) =
        ShellIntegration::install_for_agent_launch(&mut command, &launch).unwrap()
    else {
        return;
    };
    integration.start_wait(Some(std::process::id()));
    assert!(integration.waiting_for_prompt(Some(std::process::id()), 1));
}

#[test]
fn concurrent_agent_panes_own_distinct_shell_integration_directories() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut command = CommandBuilder::new("/bin/bash");
                let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
                    "BUS_LAUNCH_ID".into(),
                    "parallel-panes".into(),
                )]);
                barrier.wait();
                ShellIntegration::install_for_agent_launch(&mut command, &launch).unwrap()
            })
        })
        .collect();
    let mut integrations: Vec<_> = threads
        .into_iter()
        .filter_map(|thread| thread.join().unwrap())
        .collect();
    if integrations.is_empty() {
        return;
    }
    let directories: std::collections::BTreeSet<_> =
        integrations.iter().map(|i| i.directory.clone()).collect();
    assert_eq!(directories.len(), 8);
    drop(integrations.pop());
    assert!(integrations
        .iter()
        .all(|i| i.directory.join("bashrc").is_file()));
    drop(integrations);
    assert!(directories.iter().all(|path| !path.exists()));
}

#[test]
fn owned_powershell_panes_keep_profiles_cwd_and_verified_editor_protocol() {
    use super::super::spawn::{
        pane_shell_command_builder_for_target, PaneLaunchEnv, PaneShellConfig, ShellLaunchTarget,
    };
    for shell in [
        "powershell.exe",
        "pwsh.exe",
        "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
    ] {
        let mut cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new(shell, crate::utils::config::ShellModeConfig::NonLogin),
            ShellLaunchTarget::Windows,
        )
        .unwrap();
        let env =
            PaneLaunchEnv::from_extra(vec![]).with_identity("w1".into(), "t1".into(), "p1".into());
        let integration =
            ShellIntegration::install_for_target(&mut cmd, &env, ShellLaunchTarget::Windows)
                .unwrap()
                .unwrap();
        let args = cmd.get_argv();
        assert_eq!(args[0], shell);
        assert_eq!(args[1], "-NoExit");
        assert_eq!(args[2], "-Command");
        let script = args[3].to_string_lossy();
        assert!(
            !script.contains('"'),
            "PowerShell argv quoting changed: {script}"
        );
        assert!(!script.contains("NoProfile"));
        assert!(script.contains("[Environment]::CurrentDirectory"));
        assert!(script.contains("GetBufferState([ref]$line, [ref]$point)"));
        assert!(script.contains("RevertLine()"));
        assert!(
            !script.contains("AcceptLine"),
            "a witness must never execute input"
        );
        assert!(script.contains("Set-PSReadLineKeyHandler -Chord 'Alt+z,v'"));
        assert!(script.contains("Set-PSReadLineKeyHandler -Chord 'Alt+z,c'"));
        let probe = integration.editor_probe(None);
        assert_eq!(probe.key('v').as_ref(), b"\x1bzv");
        assert_eq!(probe.key('c').as_ref(), b"\x1bzc");
        assert!(
            integration.waiting_for_prompt(None, 1),
            "a marker without process identity cannot authorize input"
        );
    }
}

#[test]
fn powershell_editor_paths_are_literals_even_with_spaces_and_apostrophes() {
    let root = crate::utils::test_temp::unique_temp_path("powershell-literals");
    let directory = root.join("O'Connor Bus cache");
    std::fs::create_dir_all(&directory).unwrap();
    let integration = ShellIntegration {
        directory,
        token: "nonce".into(),
        shell: "powershell.exe".into(),
        windows: true,
        started_at: Instant::now(),
        process: None,
        initial_image: None,
        observed_image: RefCell::new(None),
        prompt_floor: Cell::new(0),
    };
    let script = integration.script(other_shells::POWERSHELL);
    assert!(script.contains("O''Connor Bus cache"));
    assert!(script.contains("capture'"));
    assert!(!script.contains("__BUS_"));
    drop(integration);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fish_keeps_login_and_sources_integration_after_user_config() {
    let root = crate::utils::test_temp::unique_temp_path("fish-command-fixture");
    std::fs::create_dir_all(&root).unwrap();
    let shell = root.join("fish");
    std::os::unix::fs::symlink("/bin/sh", &shell).unwrap();
    for login in [false, true] {
        let mut cmd = if login {
            CommandBuilder::new_default_prog()
        } else {
            CommandBuilder::new(&shell)
        };
        if login {
            cmd.env("SHELL", &shell);
        }
        let env = super::super::spawn::PaneLaunchEnv::from_extra(vec![]).with_identity(
            "w1".into(),
            "t1".into(),
            "p1".into(),
        );
        let integration = ShellIntegration::install_for_target(
            &mut cmd,
            &env,
            super::super::spawn::ShellLaunchTarget::OtherUnix,
        )
        .unwrap()
        .unwrap();
        let args = cmd
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(args.iter().any(|arg| *arg == "--login"), login);
        assert!(args.iter().any(|arg| *arg == "--interactive"));
        assert!(args.iter().any(|arg| *arg == "--init-command"));
        let script =
            std::fs::read_to_string(integration.directory.join("integration.fish")).unwrap();
        assert!(script.contains("commandline --current-buffer"));
        assert!(script.contains("commandline --replace ''"));
        assert!(
            script.find("__bus_original_prompt\n").unwrap()
                < script.find("printf '\\e]133;A;").unwrap()
        );
        assert!(!script.contains("commandline -f execute"));
        assert_eq!(integration.editor_probe(None).key('v').as_ref(), b"\x1bzv");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unowned_panes_are_never_modified_and_plain_sh_has_no_editor_integration() {
    for (shell, owned) in [
        ("/bin/zsh", false),
        ("powershell.exe", false),
        ("/bin/sh", true),
        ("/bin/dash", true),
    ] {
        let mut cmd = CommandBuilder::new(shell);
        let before = cmd.get_argv().to_vec();
        let mut env = super::super::spawn::PaneLaunchEnv::default();
        if owned {
            env = env.with_identity("w1".into(), "t1".into(), "p1".into());
        }
        assert!(ShellIntegration::install_for_agent_launch(&mut cmd, &env)
            .unwrap()
            .is_none());
        assert_eq!(cmd.get_argv(), &before);
    }
}

#[test]
fn managed_agent_launch_rejects_unsupported_shell_with_named_error() {
    let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
        "BUS_LAUNCH_ID".into(),
        "unsupported-shell-launch".into(),
    )]);
    for shell in ["/bin/sh", "/bin/cat"] {
        let mut command = CommandBuilder::new(shell);
        let error = ShellIntegration::install_for_agent_launch(&mut command, &launch)
            .expect_err("unsupported agent shells must fail before a pane is spawned");
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains(shell), "{error}");
    }
}
