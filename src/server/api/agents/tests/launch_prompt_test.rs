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
        executable: None,
        grok_home: None,
        kimi_home: None,
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
            "missing {text}: {}; input_error={:?}",
            runtime.visible_text(),
            runtime.launch_input_error()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_prompt(runtime: &TerminalRuntime) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while runtime.waiting_for_shell_prompt() {
        assert!(
            std::time::Instant::now() < deadline,
            "no prompt evidence: {}; {}",
            runtime.visible_text(),
            runtime.shell_prompt_diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_launch_input(runtime: &TerminalRuntime) {
    let deadline = std::time::Instant::now() + crate::protocol::api::AGENT_START_TIMEOUT;
    while !runtime.test_launch_input_submitted() {
        assert!(
            runtime.launch_input_error().is_none(),
            "launch input failed: {:?}",
            runtime.launch_input_error()
        );
        assert!(
            std::time::Instant::now() < deadline,
            "verified launch input was never submitted: {}",
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
        (
            "XDG_CONFIG_HOME".into(),
            dir.join("config").display().to_string(),
        ),
        (
            "XDG_DATA_HOME".into(),
            dir.join("data").display().to_string(),
        ),
        (
            "XDG_STATE_HOME".into(),
            dir.join("state").display().to_string(),
        ),
        (
            "XDG_CACHE_HOME".into(),
            dir.join("cache").display().to_string(),
        ),
        ("ZDOTDIR".into(), dir.display().to_string()),
        (
            "PATH".into(),
            format!("{}/bin:/usr/bin:/bin", dir.display()),
        ),
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
            wait_for_prompt(&runtime).await;
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
    wait_for_prompt(&runtime).await;
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
    let runtime = fixture_shell_with_launch_identity(
        "/bin/sh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
        false,
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
        executable: None,
        grok_home: None,
        kimi_home: None,
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
    wait_for_prompt(runtime).await;
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
async fn launch_prompt_rc_exec_without_integration_fails_closed() {
    for command in ["exec /bin/sh\n", "exec -a sh /bin/bash --norc -i\n"] {
        let dir = crate::utils::test_temp::unique_temp_path("bus-bash-exec-sh");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".bashrc"), command).unwrap();
        let runtime = fixture_shell(
            "/bin/bash",
            crate::utils::config::ShellModeConfig::NonLogin,
            &dir,
        );
        assert_unmarked_launch_is_rejected(runtime).await;
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
            b"print -r -- HAS_BUS_HOOK=$+functions[__bus_install_line_init]\r",
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
    wait_for_prompt(&runtime).await;
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

fn install_fake_provider(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let file = dir.join("bin/codex");
    std::fs::write(
        &file,
        "#!/bin/sh\nprintf 'FIXTURE_PROVIDER_RUNNING\\n'\nread -r unused\n",
    )
    .unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn app_with_prompt_runtime(runtime: TerminalRuntime) -> (App, String) {
    let mut app = app_with_agent();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.terminal_runtimes.insert(terminal_id, runtime);
    let target = app.public_pane_id(0, pane).unwrap();
    (app, target)
}

fn prompt_runtime(app: &App) -> &TerminalRuntime {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap();
    app.terminal_runtimes.get(terminal_id).unwrap()
}

fn try_prompt_launch(
    app: &mut App,
    target: &str,
) -> Result<(), crate::protocol::api::schema::ErrorBody> {
    app.start_agent(crate::protocol::api::schema::AgentStartParams {
        name: "fallback".into(),
        kind: "codex".into(),
        pane_id: target.into(),
        args: vec![
            "--no-daemon".into(),
            "-c".into(),
            "check_for_update_on_startup=false".into(),
        ],
        executable: None,
        grok_home: None,
        kimi_home: None,
        timeout_ms: None,
    })
    .map(|_| ())
    .map_err(|error| app.agent_start_error_body(error))
}

async fn assert_unmarked_launch_is_rejected(runtime: TerminalRuntime) {
    let (mut app, target) = app_with_prompt_runtime(runtime);
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(9) {
        let error = try_prompt_launch(&mut app, &target)
            .expect_err("an exec handoff without fresh prompt evidence must not launch");
        assert_eq!(error.code, "agent_shell_starting", "{error:?}");
        assert!(!prompt_runtime(&app)
            .visible_text()
            .contains("FIXTURE_PROVIDER_RUNNING"));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(available_shell_name(prompt_runtime(&app)).is_none());
}

#[tokio::test]
async fn launch_prompt_same_name_handoff_requires_fresh_replacement_evidence() {
    // Adopted reviewer probe, plus the same handoff from a login profile.
    for mode in [
        crate::utils::config::ShellModeConfig::NonLogin,
        crate::utils::config::ShellModeConfig::Login,
    ] {
        let dir = crate::utils::test_temp::unique_temp_path("bus-bash-exec-bash");
        std::fs::create_dir_all(&dir).unwrap();
        install_fake_provider(&dir);
        let (file, command) = if mode == crate::utils::config::ShellModeConfig::Login {
            (".bash_profile", "/bin/bash -l -i")
        } else {
            (".bashrc", "/bin/bash --norc -i")
        };
        std::fs::write(dir.join(file), format!("PATH=\"$HOME/bin:/usr/bin:/bin\"\nif [ -z \"$BUS_TEST_HANDOFF\" ]; then export BUS_TEST_HANDOFF=1; exec {command}; fi\nPS1='READY> '\n")).unwrap();
        let runtime = fixture_shell("/bin/bash", mode, &dir);
        assert_unmarked_launch_is_rejected(runtime).await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn launch_prompt_precmd_assignment_launches_on_the_zle_mark() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zsh-precmd-assign");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".zshrc"),
        "function my_title() { :; }\nprecmd_functions=(my_title)\nPS1='READY> '\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "READY>").await;
    wait_for_prompt(&runtime).await;
    assert!(
        runtime.launch_returned_to_prompt(),
        "precmd reset must still emit a real mark"
    );
    let (mut app, target) = app_with_prompt_runtime(runtime);
    let launch_started = std::time::Instant::now();
    try_prompt_launch(&mut app, &target).expect("ZLE readiness permits launch");
    assert!(launch_started.elapsed() < Duration::from_secs(4));
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_noisy_rc_waits_for_the_actual_prompt() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zsh-noisy-startup");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    // Builtins only: the legacy foreground-process gate already says available.
    // No prompt mark or quiet period exists during the builtin-only rc.
    std::fs::write(dir.join(".zshrc"), "precmd_functions=()\nfor i in {1..200}; do printf 'RC_PROGRESS %s\\n' $i; read -r -k 1 -t 0.05 unused; done\nPS1='READY> '\n").unwrap();
    let started = std::time::Instant::now();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "RC_PROGRESS").await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    while !prompt_runtime(&app)
        .recent_unwrapped_text(100)
        .contains("READY>")
    {
        assert!(started.elapsed() < Duration::from_secs(15));
        let error = try_prompt_launch(&mut app, &target)
            .expect_err("Bus typed while startup output was still active");
        assert_eq!(error.code, "agent_shell_starting");
        assert!(!prompt_runtime(&app)
            .recent_unwrapped_text(100)
            .contains("codex"));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(started.elapsed() >= Duration::from_secs(10));
    wait_for_prompt(prompt_runtime(&app)).await;
    assert!(
        prompt_runtime(&app).launch_returned_to_prompt(),
        "only real prompt evidence permits launch"
    );
    try_prompt_launch(&mut app, &target).expect("the completed noisy rc reaches its real prompt");
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_real_mark_launches_without_an_elapsed_gate() {
    for shell in ["/bin/bash", "/bin/zsh"] {
        if !std::path::Path::new(shell).exists() {
            continue;
        }
        let dir = crate::utils::test_temp::unique_temp_path("bus-mark-fast-path");
        std::fs::create_dir_all(&dir).unwrap();
        install_fake_provider(&dir);
        let rc = if shell.ends_with("bash") {
            ".bashrc"
        } else {
            ".zshrc"
        };
        std::fs::write(dir.join(rc), "PS1='READY> '\n").unwrap();
        let runtime = fixture_shell(shell, crate::utils::config::ShellModeConfig::NonLogin, &dir);
        wait_for_text(&runtime, "READY>").await;
        wait_for_prompt(&runtime).await;
        assert!(
            runtime.launch_returned_to_prompt(),
            "fixture must emit the mark"
        );
        let (mut app, target) = app_with_prompt_runtime(runtime);
        let launch_started = std::time::Instant::now();
        try_prompt_launch(&mut app, &target)
            .expect("real prompt evidence permits immediate launch");
        assert!(launch_started.elapsed() < Duration::from_secs(4));
        wait_for_launch_input(prompt_runtime(&app)).await;
        wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn launch_prompt_unanswered_startup_question_is_not_typed_into_after_the_fallback_window() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-unanswered-startup-question");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".zshrc"),
        "print -n 'STARTUP QUESTION [Y/n] '; read -r -k 1 answer; print; PS1='READY> '\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "STARTUP QUESTION").await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    // Adopt the R3 probe's delayed repeated calls: one call alone starts the old
    // quiet clock and misses the swallowed-keystroke bug.
    tokio::time::sleep(Duration::from_secs(9)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let error = try_prompt_launch(&mut app, &target)
            .expect_err("Bus typed the provider command into the unanswered startup question");
        assert!(
            !prompt_runtime(&app).visible_text().contains("codex"),
            "launch bytes reached the question: {}",
            prompt_runtime(&app).visible_text()
        );
        if error.code == "agent_shell_waiting" {
            assert!(
                error.message.contains("STARTUP QUESTION [Y/n]"),
                "{error:?}"
            );
            assert!(error.message.contains("30 seconds"), "{error:?}");
            assert!(error.message.contains("resume"), "{error:?}");
            break;
        }
        assert_eq!(error.code, "agent_shell_starting", "{error:?}");
        assert!(
            std::time::Instant::now() < deadline,
            "no actionable deadline error: {error:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    prompt_runtime(&app)
        .try_send_bytes(Bytes::from_static(b"n"))
        .unwrap();
    wait_for_text(prompt_runtime(&app), "READY>").await;
    wait_for_prompt(prompt_runtime(&app)).await;
    try_prompt_launch(&mut app, &target).expect("answering the question must allow one launch");
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
    assert!(!prompt_runtime(&app).visible_text().contains("odex:"));
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_zle_survives_precmd_reset_without_an_elapsed_fallback() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zle-precmd-reset");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".zshrc"), "precmd_functions=()\nPS1='READY> '\n").unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "READY>").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while runtime.waiting_for_shell_prompt() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !runtime.waiting_for_shell_prompt(),
        "the primary ZLE prompt must emit real readiness"
    );
    assert!(
        runtime.launch_returned_to_prompt(),
        "this must be actual prompt evidence"
    );
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_unrelated_osc_does_not_authorize_a_startup_question() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-unrelated-prompt-mark");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".zshrc"), "printf '\\033]133;A\\007\\033]133;A;bus=deadbeef\\007'; print -n 'UNRELATED QUESTION '; read -r -k 1 answer; print; PS1='READY> '\n").unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "UNRELATED QUESTION").await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    let error = try_prompt_launch(&mut app, &target)
        .expect_err("unrelated OSC is not our shell's readiness");
    assert_eq!(error.code, "agent_shell_starting");
    assert!(!prompt_runtime(&app).visible_text().contains("codex"));
    prompt_runtime(&app)
        .try_send_bytes(Bytes::from_static(b"n"))
        .unwrap();
    wait_for_prompt(prompt_runtime(&app)).await;
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_input_invalidates_evidence_before_same_or_changed_image_exec() {
    for shell in ["/bin/bash", "/bin/zsh"] {
        if !std::path::Path::new(shell).exists() {
            continue;
        }
        let dir = crate::utils::test_temp::unique_temp_path("bus-prompt-then-exec");
        std::fs::create_dir_all(&dir).unwrap();
        let rc = if shell.ends_with("zsh") {
            ".zshrc"
        } else {
            ".bashrc"
        };
        std::fs::write(dir.join(rc), "export PS1='READY> '\n").unwrap();
        let runtime = fixture_shell(shell, crate::utils::config::ShellModeConfig::NonLogin, &dir);
        wait_for_prompt(&runtime).await;
        let pid = runtime.child_pid().unwrap();
        runtime
            .try_send_bytes(Bytes::from_static(
                b"PS1='REPLACEMENT_\\s> ' exec /bin/bash --noprofile --norc -i\r",
            ))
            .unwrap();
        wait_for_text(&runtime, "REPLACEMENT_bash>").await;
        assert_eq!(runtime.child_pid(), Some(pid), "exec keeps the shell PID");
        let (mut app, target) = app_with_prompt_runtime(runtime);
        let error = try_prompt_launch(&mut app, &target)
            .expect_err("pre-exec prompt evidence must not be reused");
        assert_eq!(error.code, "agent_shell_starting");
        assert!(!prompt_runtime(&app).visible_text().contains("codex"));
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn launch_prompt_startup_vared_and_late_user_widget_keep_real_readiness() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zle-vared-widget");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".zshrc"), "precmd_functions=()\nanswer=''; vared -p 'VARED QUESTION> ' answer\nmy_init() { print -r -- USER_WIDGET:$WIDGET:$KEYMAP; return 7; }\nzle -N zle-line-init my_init\nPS1='READY> '\n").unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_text(&runtime, "VARED QUESTION>").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        runtime.waiting_for_shell_prompt(),
        "vared is not the primary prompt"
    );
    assert!(!runtime.launch_returned_to_prompt());
    runtime.try_send_bytes(Bytes::from_static(b"n\r")).unwrap();
    wait_for_prompt(&runtime).await;
    let text = runtime.visible_text();
    assert!(text.contains("USER_WIDGET:zle-line-init:"), "{text}");
    assert_eq!(
        text.matches("USER_WIDGET:").count(),
        1,
        "user init must run exactly once: {text}"
    );
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_zle_preserves_user_status_and_widget_context() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-zle-user-status");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".zshrc"), "my_init() { local rc=$?; print -r -- USER_WIDGET_STATUS:$rc:$WIDGET; return 7; }\nzle -N zle-line-init my_init\nPS1='READY_STATUS=%?> '\nfalse\n").unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let text = runtime.visible_text();
    assert!(text.contains("READY_STATUS=1>"), "{text}");
    assert!(
        text.contains("USER_WIDGET_STATUS:1:zle-line-init"),
        "{text}"
    );
    runtime
        .try_send_bytes(Bytes::from_static(b"false\r"))
        .unwrap();
    wait_for_prompt(&runtime).await;
    let text = runtime.visible_text();
    assert_eq!(
        text.matches("USER_WIDGET_STATUS:1:zle-line-init").count(),
        2,
        "{text}"
    );
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_automatic_image_handoff_rejects_the_old_prompt_without_input() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("bus-automatic-shell-exec");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".zshrc"),
        "TRAPUSR1() { export PS1='SIGNAL_REPLACEMENT_\\s> '; exec /bin/bash --noprofile --norc -i; }\nPS1='READY> '\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let pid = runtime.child_pid().unwrap();
    let old_image = crate::platform::process_image(pid).expect("known initial image");
    // An external signal changes the image without any pane input invalidation.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR1) }, 0);
    wait_for_text(&runtime, "SIGNAL_REPLACEMENT_bash>").await;
    assert_eq!(runtime.child_pid(), Some(pid));
    assert_ne!(crate::platform::process_image(pid), Some(old_image));
    assert!(
        runtime.launch_returned_to_prompt(),
        "old mark is retained until the identity check"
    );
    let (mut app, target) = app_with_prompt_runtime(runtime);
    let error = try_prompt_launch(&mut app, &target)
        .expect_err("replacement cannot reuse the old image's mark");
    assert_eq!(error.code, "agent_shell_starting");
    assert!(!prompt_runtime(&app).visible_text().contains("codex"));
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn launch_prompt_relative_shell_uses_the_panes_cwd_for_image_proof() {
    for shell in ["bash", "zsh"] {
        let program = std::path::Path::new("/bin").join(shell);
        if !program.exists() {
            continue;
        }
        let dir = crate::utils::test_temp::unique_temp_path("bus-relative-shell-image");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        // Use the real shell through a symlink: a copied system binary can be
        // killed by macOS before this pane-cwd resolution assertion runs.
        std::os::unix::fs::symlink(program, dir.join("bin").join(shell)).unwrap();
        let rc = if shell == "zsh" { ".zshrc" } else { ".bashrc" };
        std::fs::write(dir.join(rc), "PS1='PATH_READY> '\n").unwrap();
        let runtime = fixture_shell(
            &format!("./bin/{shell}"),
            crate::utils::config::ShellModeConfig::NonLogin,
            &dir,
        );
        wait_for_prompt(&runtime).await;
        assert!(runtime.launch_returned_to_prompt());
        assert!(available_shell_name(&runtime).is_some());
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn startup_snapshot_preserves_blank_rows_osc_and_native_identity() {
    let mut app = app_with_agent();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        AgentKind::Codex,
        std::time::Instant::now(),
        Duration::ZERO,
        Duration::from_secs(30),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    let (runtime, _writes) = TerminalRuntime::test_with_channel(80, 24);
    runtime.test_process_pty_bytes("\x1b]2;⠋ Working\x07\x1b]9;4;1;\x07› Ask Codex to do anything\r\n\r\n  GPT-6.1-Sol xhigh · ~/scratch\r\n  ? for shortcuts".as_bytes());
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let target = app.public_pane_id(0, pane).unwrap();
    let request = |expected: &str| {
        serde_json::from_value::<crate::protocol::api::schema::Request>(serde_json::json!({
            "id":"diagnostics", "method":"agent.startup_snapshot",
            "params":{"pane_id":target,"expected_terminal_id":expected}
        }))
        .expect("startup failure must support a guarded native snapshot")
    };
    let response = app.handle_api_request(request(terminal_id.as_str()));
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    let snapshot = &value["result"]["snapshot"];
    assert_eq!(snapshot["pane_id"], target);
    assert_eq!(snapshot["terminal_id"], terminal_id.as_str());
    assert_eq!(snapshot["native_agent"], "codex");
    assert_eq!(snapshot["native_name"], "bus-r1-a2");
    assert_eq!(snapshot["native_status"], "working");
    assert_eq!(snapshot["osc_title"], "⠋ Working");
    assert_eq!(snapshot["osc_progress"], "4;1;");
    assert!(snapshot["screen"]
        .as_str()
        .unwrap()
        .contains("› Ask Codex to do anything\n\n  GPT-6.1-Sol"));
    let changed: serde_json::Value =
        serde_json::from_str(&app.handle_api_request(request("replacement"))).unwrap();
    assert_eq!(changed["error"]["code"], "agent_identity_changed");
    assert!(!changed.to_string().contains("Ask Codex"));
}

#[tokio::test]
async fn startup_snapshot_refuses_a_bound_conversation_screen() {
    let (app, params, _writes) = claude_agent_with_input("PRIVATE_CONVERSATION_CONTENT");
    let response = app.handle_agent_startup_snapshot(
        "privacy".into(),
        crate::protocol::api::schema::AgentStartupSnapshotParams {
            pane_id: params.expected_pane_id,
            expected_terminal_id: params.expected_terminal_id,
        },
    );
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["error"]["code"], "startup_snapshot_unavailable");
    assert!(!response.contains("PRIVATE_CONVERSATION_CONTENT"));
}

#[tokio::test]
async fn startup_snapshot_does_not_capture_an_unfinished_redraw() {
    let runtime = TerminalRuntime::test_with_screen_bytes(80, 24, b"startup");
    runtime.test_process_pty_bytes(b"\x1b[?2026hpartial");
    assert!(runtime.startup_snapshot().is_none());
    runtime.test_process_pty_bytes(b"\x1b[?2026l");
    assert!(runtime.startup_snapshot().unwrap().0.contains("partial"));
}

#[path = "verified_launch_test.rs"]
mod verified_launch;

#[cfg(unix)]
#[path = "kimi_wire_launch_test.rs"]
mod kimi_wire_launch;
