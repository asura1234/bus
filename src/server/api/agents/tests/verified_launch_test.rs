use super::*;

#[tokio::test]
async fn verified_launch_corrects_first_byte_loss_after_slow_precmd() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    for resume in [false, true] {
        let dir = crate::utils::test_temp::unique_temp_path("verified-launch");
        std::fs::create_dir_all(&dir).unwrap();
        install_fake_provider(&dir);
        std::fs::write(dir.join(".zshrc"), "precmd() { export PROMPT_ENV=ready; sleep 0.2; }\nfunction drop_first() { print dropped >> \"$HOME/dropped\"; zle .bracketed-paste; BUFFER=${BUFFER[2,-1]}; }\nzle -N bracketed-paste drop_first\nPS1='ready> '\n").unwrap();
        std::fs::write(dir.join("bin/codex"), "#!/bin/sh\nprintf '%s\\n' \"$PROMPT_ENV\" > \"$HOME/environment\"\nprintf '%s\\n' \"$@\" > \"$HOME/arguments\"\nprintf 'FIXTURE_PROVIDER_RUNNING\\n'\nread -r unused\n").unwrap();
        let runtime = fixture_shell(
            "/bin/zsh",
            crate::utils::config::ShellModeConfig::Login,
            &dir,
        );
        wait_for_prompt(&runtime).await;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !runtime.bracketed_paste_enabled() {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (mut app, target) = app_with_prompt_runtime(runtime);
        let args = if resume {
            vec![
                "--resume".into(),
                "saved-session".into(),
                "one two".into(),
                "中文".into(),
            ]
        } else {
            vec!["one two".into(), "中文".into()]
        };
        app.start_agent(crate::protocol::api::schema::AgentStartParams {
            name: "verified".into(),
            kind: "codex".into(),
            pane_id: target,
            args: args.clone(),
            timeout_ms: None,
            executable: None,
            grok_home: None,
            kimi_home: None,
        })
        .unwrap_or_else(|error| panic!("{:?}", app.agent_start_error_body(error)));
        wait_for_launch_input(prompt_runtime(&app)).await;
        wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
        assert!(!prompt_runtime(&app)
            .recent_unwrapped_text(100)
            .contains("command not found"));
        assert_eq!(
            std::fs::read_to_string(dir.join("environment")).unwrap(),
            "ready\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("dropped")).unwrap(),
            "dropped\n"
        );
        let actual = std::fs::read_to_string(dir.join("arguments")).unwrap();
        assert!(
            actual.starts_with(&format!("{}\n", args.join("\n"))),
            "{actual}"
        );
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn verified_launch_preserves_bash_function_and_post_exit_shell() {
    let dir = crate::utils::test_temp::unique_temp_path("verified-bash");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(dir.join(".bashrc"), "PROMPT_COMMAND='export PROMPT_ENV=ready'\ncodex() { export FUNCTION_ENV=wrapper; command codex \"$@\"; }\nPS1='POST_EXIT_READY> '\n").unwrap();
    std::fs::write(dir.join(".bash_profile"), "source \"$HOME/.bashrc\"\n").unwrap();
    std::fs::write(dir.join("bin/codex"), "#!/bin/sh\nprintf '%s|%s\\n' \"$PROMPT_ENV\" \"$FUNCTION_ENV\" > \"$HOME/environment\"\nprintf 'PROVIDER_FATAL_AUTH_ERROR\\n'\nexit 1\n").unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::Login,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    try_prompt_launch(&mut app, &target).unwrap();
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "PROVIDER_FATAL_AUTH_ERROR").await;
    assert_eq!(
        std::fs::read_to_string(dir.join("environment")).unwrap(),
        "ready|wrapper\n"
    );
    assert!(crate::platform::process_instance(prompt_runtime(&app).child_pid().unwrap()).is_some());
    wait_for_prompt(prompt_runtime(&app)).await;
    let proof = dir.join("post-exit-proof");
    let command = format!("printf usable > '{}'\r", proof.display());
    prompt_runtime(&app)
        .try_send_bytes(Bytes::from(command))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !proof.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "post-exit shell did not execute input"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(std::fs::read_to_string(proof).unwrap(), "usable");
    assert!(app
        .find_pane(app.state.workspaces[0].tabs[0].root_pane)
        .is_some());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_integrates_api_created_panes_without_a_launch_id() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("api-editor");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".zshrc"),
        "precmd() { sleep 0.2; }\nPS1='READY> '\n",
    )
    .unwrap();
    let mut app = app_with_agent();
    app.state.default_shell = "/bin/zsh".into();
    let params=serde_json::from_value(serde_json::json!({
        "cwd":dir,"env":{"HOME":dir,"ZDOTDIR":dir,"PATH":format!("{}/bin:/usr/bin:/bin",dir.display())}
    })).unwrap();
    let response = app.handle_tab_create("pane".into(), params);
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    let target = value["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let runtime = app
        .terminal_runtimes
        .values()
        .find(|runtime| runtime.has_launch_shell_integration())
        .unwrap();
    wait_for_prompt(runtime).await;
    try_prompt_launch(&mut app, &target).unwrap();
    let runtime = app
        .terminal_runtimes
        .values()
        .find(|runtime| runtime.has_launch_shell_integration())
        .unwrap();
    wait_for_launch_input(runtime).await;
    wait_for_text(runtime, "FIXTURE_PROVIDER_RUNNING").await;
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_refuses_an_unowned_unintegrated_pane_before_input() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("unowned-editor");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(dir.join(".zshrc"), "PS1='UNOWNED_READY> '\n").unwrap();
    let runtime = fixture_shell_with_launch_identity(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
        false,
    );
    wait_for_text(&runtime, "UNOWNED_READY>").await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    let error = try_prompt_launch(&mut app, &target).unwrap_err();
    assert!(
        error.message.contains("no launch input was sent"),
        "{error:?}"
    );
    assert!(!prompt_runtime(&app)
        .recent_unwrapped_text(100)
        .contains("codex"));
    assert!(!prompt_runtime(&app).has_launch_shell_integration());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_never_enters_a_line_transformed_twice() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("unverified-launch");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(dir.join(".zshrc"), "function drop_first() { zle .bracketed-paste; BUFFER=${BUFFER[2,-1]}; }\nzle -N bracketed-paste drop_first\nfunction self-insert() { zle .self-insert; BUFFER=${BUFFER#c}; }\nzle -N self-insert\nfunction record_enter() { print entered >> \"$HOME/enters\"; zle .accept-line; }\nzle -N accept-line record_enter\nPS1='READY> '\n").unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    try_prompt_launch(&mut app, &target).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while prompt_runtime(&app).launch_input_error().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "broken editor did not reject launch"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(prompt_runtime(&app)
        .launch_input_error()
        .unwrap()
        .contains("twice"));
    let (ws, pane) = app.parse_current_public_pane_id(&target).unwrap();
    let info = serde_json::to_value(app.agent_info(ws, pane).unwrap()).unwrap();
    assert!(
        info["launch_input_error"]
            .as_str()
            .unwrap_or("")
            .contains("twice"),
        "{info}"
    );
    // A fresh editor round trip drains every earlier write before checking
    // the accept-line counter; absence is not an arbitrary timed sleep.
    assert!(prompt_runtime(&app)
        .test_capture_shell_editor()
        .unwrap()
        .is_empty());
    assert!(
        !dir.join("enters").exists(),
        "Enter reached a damaged launch line"
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_cold_restore_allows_startup_answers_before_editor_readiness() {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("restore-editor-question");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".zshrc"),
        "print -n 'STARTUP QUESTION '; read -r -k 1 answer; print; PS1='READY> '\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/zsh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    runtime
        .queue_shell_command(&["codex".into()], Duration::from_secs(5))
        .unwrap();
    wait_for_text(&runtime, "STARTUP QUESTION").await;
    assert!(!runtime.visible_text().contains("FIXTURE_PROVIDER_RUNNING"));
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"y"))
        .unwrap();
    wait_for_text(&runtime, "FIXTURE_PROVIDER_RUNNING").await;
    assert!(runtime.launch_input_error().is_none());
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_plain_sh_fails_before_input_with_supported_shell_guidance() {
    let dir = crate::utils::test_temp::unique_temp_path("plain-shell-editor");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    let runtime = fixture_shell_with_launch_identity(
        "/bin/sh",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
        false,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while available_shell_name(&runtime).is_none() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (mut app, target) = app_with_prompt_runtime(runtime);
    let error = try_prompt_launch(&mut app, &target).unwrap_err();
    assert_eq!(error.code, "agent_shell_integration_unavailable");
    for shell in ["zsh", "bash", "fish", "PowerShell"] {
        assert!(error.message.contains(shell));
    }
    assert!(!prompt_runtime(&app)
        .recent_unwrapped_text(100)
        .contains("codex"));
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_fish_preserves_prompt_function_arguments_and_shell() {
    let dir = crate::utils::test_temp::unique_temp_path("fish-editor");
    let Some(shell) = std::env::var_os("PATH").and_then(|path| {
        crate::platform::resolve_executable_image(
            std::path::Path::new("fish"),
            Some(&path),
            &std::env::current_dir().unwrap(),
        )
    }) else {
        return;
    };
    std::fs::create_dir_all(dir.join("config/fish")).unwrap();
    install_fake_provider(&dir);
    std::fs::write(dir.join("config/fish/config.fish"),"function fish_prompt\n set -gx PROMPT_ENV ready\n printf 'FISH_READY> '\nend\nfunction codex\n set -gx FUNCTION_ENV wrapper\n command codex $argv\nend\n").unwrap();
    std::fs::write(dir.join("bin/codex"),"#!/bin/sh\nprintf '%s|%s\\n' \"$PROMPT_ENV\" \"$FUNCTION_ENV\" > \"$HOME/environment\"\nprintf '%s\\n' \"$@\" > \"$HOME/arguments\"\nprintf 'FISH_PROVIDER_EXIT\\n'\nexit 1\n").unwrap();
    let runtime = fixture_shell(
        shell.to_str().unwrap(),
        crate::utils::config::ShellModeConfig::Login,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    // A fresh fish HOME also generates man-page completions asynchronously.
    // Wait for the API's idle-shell precondition, not just prompt rendering.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while crate::platform::available_pane_shell(runtime.child_pid().unwrap()).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "fish startup remained busy"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (mut app, target) = app_with_prompt_runtime(runtime);
    app.start_agent(crate::protocol::api::schema::AgentStartParams {
        name: "fish-worker".into(),
        kind: "codex".into(),
        pane_id: target,
        args: vec![
            "one two".into(),
            "quote'and;$()".into(),
            "中文".into(),
            String::new(),
        ],
        timeout_ms: None,
        executable: None,
        grok_home: None,
        kimi_home: None,
    })
    .unwrap_or_else(|error| {
        panic!(
            "{:?}; foreground={:?}; screen={}",
            app.agent_start_error_body(error),
            crate::platform::foreground_job(prompt_runtime(&app).child_pid().unwrap()),
            prompt_runtime(&app).visible_text()
        )
    });
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "FISH_PROVIDER_EXIT").await;
    assert_eq!(
        std::fs::read_to_string(dir.join("environment")).unwrap(),
        "ready|wrapper\n"
    );
    assert!(std::fs::read_to_string(dir.join("arguments"))
        .unwrap()
        .starts_with("one two\nquote'and;$()\n中文\n\n"));
    wait_for_prompt(prompt_runtime(&app)).await;
    assert!(crate::platform::process_instance(prompt_runtime(&app).child_pid().unwrap()).is_some());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
