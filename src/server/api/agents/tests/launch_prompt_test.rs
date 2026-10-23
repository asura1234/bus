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
        &PaneLaunchEnv::from_extra(vec![
            ("HOME".into(), dir.display().to_string()),
            ("ZDOTDIR".into(), dir.display().to_string()),
        ]),
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
        &PaneLaunchEnv::from_extra(vec![("ZDOTDIR".into(), dir.display().to_string())]),
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
        Err(err) => assert_eq!(app.agent_start_error_body(err).code, "agent_pane_busy"),
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
