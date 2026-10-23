use super::*;
use crate::server::terminals::agents::available_shell_name;
use crate::terminal::runtime::spawn::{PaneLaunchEnv, PaneShellConfig};
use crate::terminal::TerminalRuntime;

#[tokio::test]
async fn launch_prompt_return_without_provider_releases_managed_launch() {
    let mut app = app_with_agent();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, _writes) = TerminalRuntime::test_with_channel(80, 24);
    runtime.test_process_pty_bytes(b"\x1b]133;A\x07ready> ");
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let target = app.public_pane_id(0, pane).unwrap();
    app.start_agent(crate::protocol::api::schema::AgentStartParams {
        name: "missing".into(),
        kind: "codex".into(),
        pane_id: target,
        args: vec![],
        timeout_ms: None,
    })
    .unwrap_or_else(|err| {
        panic!(
            "initial launch should be accepted: {}",
            app.agent_start_error_body(err).message
        )
    });
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.terminal_runtimes
        .get(&terminal_id)
        .unwrap()
        .test_process_pty_bytes(b"zsh: command not found: codex\r\n\x1b]133;A\x07ready> ");
    app.handle_agent_list("poll".into());
    assert!(
        !app.state.terminals[&terminal_id].managed_agent_launch_pending(),
        "shell returned to its prompt but provider is still Launching"
    );
}

async fn wait_for_text(runtime: &TerminalRuntime, text: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !runtime.recent_unwrapped_text(100).contains(text) {
        assert!(
            std::time::Instant::now() < deadline,
            "missing {text}: {}",
            runtime.visible_text()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn fixture_shell(
    shell: &str,
    mode: crate::utils::config::ShellModeConfig,
    dir: &std::path::Path,
) -> TerminalRuntime {
    fixture_shell_with_launch_identity(shell, mode, dir, true)
}

fn fixture_shell_with_launch_identity(
    shell: &str,
    mode: crate::utils::config::ShellModeConfig,
    dir: &std::path::Path,
    agent_launch: bool,
) -> TerminalRuntime {
    let mut extra = vec![
        ("HOME".into(), dir.display().to_string()),
        ("ZDOTDIR".into(), dir.display().to_string()),
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("PROMPT_COMMAND".into(), String::new()),
        ("ENV".into(), String::new()),
        ("BASH_ENV".into(), String::new()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
    ];
    if agent_launch {
        extra.push(("BUS_LAUNCH_ID".into(), "fixture-launch".into()));
    }
    let launch_env = PaneLaunchEnv::from_extra(extra);
    let (events, _rx) = tokio::sync::mpsc::channel(64);
    TerminalRuntime::spawn(
        crate::utils::ids::PaneId::from_raw(0),
        24,
        100,
        dir.into(),
        65536,
        Default::default(),
        None,
        PaneShellConfig::new(shell, mode),
        &launch_env,
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::default()),
    )
    .unwrap()
}

#[tokio::test]
async fn launch_prompt_bash_waits_through_builtin_read_and_preserves_user_prompt_commands() {
    use crate::utils::config::ShellModeConfig::{Login, NonLogin};
    for mode in [Login, NonLogin] {
        for prompt in [
            "PROMPT_COMMAND='printf USER_HOOK; '",
            "PROMPT_COMMAND=('printf USER_HOOK')",
        ] {
            let dir = crate::utils::test_temp::unique_temp_path("bus-bash-prompt");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".bashrc"), format!("printf 'BASH QUESTION '; read -r -n 1 answer; printf '\\n'; {prompt}\nPS1='READY> '\n")).unwrap();
            std::fs::write(dir.join(".bash_profile"), "source \"$HOME/.bashrc\"\n").unwrap();
            let runtime = fixture_shell("/bin/bash", mode, &dir);
            wait_for_text(&runtime, "BASH QUESTION").await;
            assert!(runtime.waiting_for_shell_prompt());
            assert!(available_shell_name(&runtime).is_none());
            runtime.try_send_bytes(Bytes::from_static(b"n")).unwrap();
            wait_for_text(&runtime, "READY>").await;
            assert!(
                !runtime.waiting_for_shell_prompt(),
                "{mode:?}, {prompt}: {}",
                runtime.visible_text()
            );
            assert!(runtime.visible_text().contains("USER_HOOK"));
            drop(runtime);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[tokio::test]
async fn launch_prompt_zsh_login_honors_zdotdir_and_waits_for_zlogin() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zsh-login");
    let dotdir = dir.join("user dotfiles");
    std::fs::create_dir_all(&dotdir).unwrap();
    std::fs::write(
        dir.join(".zshenv"),
        "ZDOTDIR=\"$HOME/user dotfiles\"\nprint USER_ENV\n",
    )
    .unwrap();
    std::fs::write(dotdir.join(".zprofile"), "print USER_PROFILE\n").unwrap();
    std::fs::write(
        dotdir.join(".zshrc"),
        "print USER_RC\nprecmd() { print USER_HOOK; }\nPS1='READY> '\n",
    )
    .unwrap();
    std::fs::write(
        dotdir.join(".zlogin"),
        "print -n 'LOGIN QUESTION '; read -r -k 1 answer; print\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::Login,
        &dir,
    );
    wait_for_text(&runtime, "LOGIN QUESTION").await;
    assert!(runtime.waiting_for_shell_prompt());
    assert!(available_shell_name(&runtime).is_none());
    runtime.try_send_bytes(Bytes::from_static(b"n")).unwrap();
    wait_for_text(&runtime, "READY>").await;
    assert!(!runtime.waiting_for_shell_prompt());
    let text = runtime.visible_text();
    for expected in ["USER_ENV", "USER_PROFILE", "USER_RC", "USER_HOOK"] {
        assert!(text.contains(expected), "{text}");
    }
    runtime
        .try_send_bytes(Bytes::from_static(b"printf 'DOTDIR=%s\\n' \"$ZDOTDIR\"\r"))
        .unwrap();
    wait_for_text(&runtime, &format!("DOTDIR={}", dotdir.display())).await;
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_unsupported_shell_keeps_process_gate() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-sh-prompt");
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = fixture_shell(
        "/bin/sh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while available_shell_name(&runtime).is_none() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!runtime.waiting_for_shell_prompt());
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_builtin_read_cannot_swallow_provider_command() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-launch-prompt");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".zshrc"),
        "print -n 'STARTUP QUESTION [Y/n] '; read -r -k 1 answer; print; PS1='READY> '\n",
    )
    .unwrap();
    let mut app = app_with_agent();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (events, _rx) = tokio::sync::mpsc::channel(64);
    let runtime = TerminalRuntime::spawn(
        pane,
        24,
        80,
        dir.clone(),
        65536,
        Default::default(),
        None,
        PaneShellConfig::new("/bin/zsh", crate::utils::config::ShellModeConfig::NonLogin),
        &PaneLaunchEnv::from_extra(vec![
            ("ZDOTDIR".into(), dir.display().to_string()),
            ("BUS_LAUNCH_ID".into(), "fixture-launch".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ]),
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::default()),
    )
    .unwrap();
    wait_for_text(&runtime, "STARTUP QUESTION").await;
    // This is the RCA's false positive: read is a builtin, with no child job.
    assert!(crate::platform::available_pane_shell(runtime.child_pid().unwrap()).is_some());
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.terminal_runtimes.insert(terminal_id, runtime);
    let target = app.public_pane_id(0, pane).unwrap();
    let result = app.start_agent(crate::protocol::api::schema::AgentStartParams {
        name: "early".into(),
        kind: "codex".into(),
        pane_id: target,
        args: vec![],
        timeout_ms: None,
    });
    match result {
        Ok(_) => panic!("Bus typed a provider command into the startup read"),
        Err(err) => assert_eq!(app.agent_start_error_body(err).code, "agent_shell_starting"),
    }
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap();
    let runtime = app.terminal_runtimes.get(terminal_id).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!runtime.visible_text().contains("codex"));
    assert!(!runtime.visible_text().contains("odex"));
    runtime.try_send_bytes(Bytes::from_static(b"n")).unwrap();
    wait_for_text(runtime, "READY>").await;
    assert!(available_shell_name(runtime).is_some());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_zsh_integration_keeps_the_users_history_file() {
    if !std::path::Path::new("/bin/zsh").exists() || !std::path::Path::new("/etc/zshrc").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zsh-history");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".zshrc"), "PS1='READY> '\n").unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::Login,
        &dir,
    );
    wait_for_text(&runtime, "READY>").await;
    runtime
        .try_send_bytes(Bytes::from_static(b"printf 'HIST=%s;\\n' \"$HISTFILE\"\r"))
        .unwrap();
    wait_for_text(&runtime, "HIST=").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let text = runtime.recent_unwrapped_text(100);
    let expected = format!("HIST={};", dir.join(".zsh_history").display());
    assert!(
        text.contains(&expected),
        "history must stay in the user's ZDOTDIR, not the deleted shim directory: {text}"
    );
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_rc_that_execs_another_shell_still_becomes_available() {
    for command in ["exec /bin/sh\n", "exec -a sh /bin/bash --norc -i\n"] {
        let dir = crate::utils::test_temp::unique_temp_path("bus-bash-exec-sh");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".bashrc"), command).unwrap();
        let runtime = fixture_shell(
            "/bin/bash",
            crate::utils::config::ShellModeConfig::NonLogin,
            &dir,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while available_shell_name(&runtime).is_none() {
            assert!(
            std::time::Instant::now() < deadline,
            "a bashrc that execs /bin/sh never reaches an available shell: waiting_for_shell_prompt={}, job={:?}",
            runtime.waiting_for_shell_prompt(), crate::agents::foreground_job(runtime.child_pid().unwrap())
        );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[tokio::test]
async fn launch_prompt_ordinary_user_pane_has_no_injected_hook() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-user-zsh");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".zshrc"), "PS1='READY> '\n").unwrap();
    let runtime = fixture_shell_with_launch_identity(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::Login,
        &dir,
        false,
    );
    wait_for_text(&runtime, "READY>").await;
    runtime
        .try_send_bytes(Bytes::from_static(
            b"print -r -- HAS_BUS_HOOK=$+functions[__bus_prompt_ready]\r",
        ))
        .unwrap();
    wait_for_text(&runtime, "HAS_BUS_HOOK=0").await;
    assert!(
        runtime
            .recent_unwrapped_text(100)
            .contains("HAS_BUS_HOOK=0"),
        "{}",
        runtime.recent_unwrapped_text(100)
    );
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_exported_bash_prompt_does_not_leak_into_child_shell() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-bash-child");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".bashrc"),
        "export PROMPT_COMMAND='printf USER_HOOK'\nexport PS1='READY> '\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "READY>").await;
    runtime
        .try_send_bytes(Bytes::from_static(b"/bin/bash --noprofile --norc -i\r"))
        .unwrap();
    runtime
        .try_send_bytes(Bytes::from_static(b"printf 'NESTED_%s\\n' READY\r"))
        .unwrap();
    wait_for_text(&runtime, "NESTED_READY").await;
    let text = runtime.recent_unwrapped_text(100);
    assert!(
        !text.contains("__bus_prompt_ready: command not found"),
        "{text}"
    );
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_seven_second_builtin_rc_waits_without_typing() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-slow-zsh");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".zshrc"),
        "print SLOW_RC; read -r -k 1 -t 7 unused; PS1='READY> '\n",
    )
    .unwrap();
    let started = std::time::Instant::now();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "SLOW_RC").await;
    assert!(crate::platform::available_pane_shell(runtime.child_pid().unwrap()).is_some());
    assert!(available_shell_name(&runtime).is_none());
    while !runtime.recent_unwrapped_text(100).contains("READY>") {
        assert!(started.elapsed() < Duration::from_secs(12));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(started.elapsed() >= Duration::from_secs(7));
    assert!(available_shell_name(&runtime).is_some());
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn launch_prompt_bash_login_limitation_is_confined_to_agent_panes() {
    for agent_launch in [false, true] {
        let dir = crate::utils::test_temp::unique_temp_path("bus-bash-login-mode");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".bash_profile"),
            "printf p >>\"$HOME/profile-count\"\nPS1='READY> '\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(".bash_logout"),
            "printf logout >\"$HOME/logout-ran\"\n",
        )
        .unwrap();
        let runtime = fixture_shell_with_launch_identity(
            "/bin/bash",
            crate::utils::config::ShellModeConfig::Login,
            &dir,
            agent_launch,
        );
        wait_for_text(&runtime, "READY>").await;
        runtime.try_send_bytes(Bytes::from_static(b"if shopt -q login_shell; then printf 'LOGIN_%s\\n' ON; else printf 'LOGIN_%s\\n' OFF; fi\r")).unwrap();
        wait_for_text(
            &runtime,
            if agent_launch {
                "LOGIN_OFF"
            } else {
                "LOGIN_ON"
            },
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(dir.join("profile-count")).unwrap(),
            "p"
        );
        let pid = runtime.child_pid().unwrap();
        runtime
            .try_send_bytes(Bytes::from_static(b"exit\r"))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while crate::platform::process_instance(pid).is_some() {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(dir.join("logout-ran").exists(), !agent_launch);
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
