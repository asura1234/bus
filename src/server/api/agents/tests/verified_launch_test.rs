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
    let command = format!(
        ": > '{0}'; sleep 0.2; printf usable > '{0}'\r",
        proof.display()
    );
    prompt_runtime(&app)
        .try_send_bytes(Bytes::from(command))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::fs::read_to_string(&proof).unwrap_or_default() != "usable" {
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
    std::fs::write(dir.join(".zshrc"), "function drop_first() { zle .bracketed-paste; BUFFER=${BUFFER[2,-1]}; }\nzle -N bracketed-paste drop_first\nfunction self-insert() { zle .self-insert; BUFFER=${BUFFER#[_c]}; }\nzle -N self-insert\nfunction record_enter() { print entered >> \"$HOME/enters\"; zle .accept-line; }\nzle -N accept-line record_enter\nPS1='READY> '\n").unwrap();
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

// Hold the actual writer at its transaction boundary, rather than hoping a
// loaded machine schedules agent.list between command capture and Enter.
#[tokio::test]
async fn verified_launch_redraw_before_enter_retains_native_ownership() {
    let dir = crate::utils::test_temp::unique_temp_path("launch-redraw");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(dir.join(".bashrc"), "PS1='READY> '\n").unwrap();
    std::fs::write(dir.join("bin/codex"), "#!/bin/sh\nprintf started > \"$HOME/started\"\nprintf 'FIXTURE_PROVIDER_RUNNING\\n'\nread -r unused\n").unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let marker = runtime.test_shell_prompt_marker();
    let barrier = runtime.test_hold_launch_input(false);
    struct Release(std::sync::Arc<crate::terminal::runtime::LaunchTestBarrier>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let release = Release(barrier.clone());
    let (mut app, target) = app_with_prompt_runtime(runtime);
    app.start_agent(crate::protocol::api::schema::AgentStartParams {
        name: "redraw".into(),
        kind: "codex".into(),
        pane_id: target.clone(),
        args: vec!["x".repeat(600)],
        timeout_ms: None,
        executable: None,
        grok_home: None,
        kimi_home: None,
    })
    .unwrap_or_else(|error| panic!("{:?}", app.agent_start_error_body(error)));
    wait_for_signal(
        || barrier.reached(),
        || {
            format!(
                "writer never reached barrier: {:?}",
                prompt_runtime(&app).launch_input_error()
            )
        },
    )
    .await;
    assert!(!dir.join("started").exists());
    let child = prompt_runtime(&app).child_pid().unwrap();
    wait_for_signal(
        || crate::platform::available_pane_shell(child).is_some(),
        || "shell was not foreground while writer held".to_owned(),
    )
    .await;
    assert!(!prompt_runtime(&app).test_launch_input_submitted());
    // Deliver the delayed rendering of the *same* primary prompt, exactly
    // where the old writer has already armed its generation floor.
    prompt_runtime(&app).test_process_pty_bytes(&marker);
    app.handle_agent_list("racing-poll".into());
    let info = app
        .agent_info_for_target(&target)
        .unwrap_or_else(|error| panic!("redraw erased launch identity: {error:?}"));
    assert_eq!(
        info.name.as_deref(),
        Some("redraw"),
        "redraw erased launch identity: {info:?}"
    );
    assert!(info.launch_pending);
    drop(release);
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
    assert_eq!(
        app.agent_info_for_target(&target).unwrap().name.as_deref(),
        Some("redraw")
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn verified_launch_bash_slow_ps1_cannot_authorize_input() {
    let dir = crate::utils::test_temp::unique_temp_path("launch-slow-ps1");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".bashrc"),
        r#"slow_prompt() {
    printf held > "$HOME/ps1-held"
    while [[ ! -e $HOME/ps1-release ]]; do read -r -t 0.01 unused; done
    printf READY
}
PS1='$(slow_prompt)> '
"#,
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    runtime
        .queue_shell_command(&["codex".into()], Duration::from_secs(5))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !dir.join("ps1-held").exists() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(runtime.waiting_for_shell_prompt());
    assert!(!runtime.test_launch_input_submitted());
    assert!(!runtime.launch_returned_to_prompt());
    assert!(!runtime.visible_text().contains("codex"));
    std::fs::write(dir.join("ps1-release"), "go").unwrap();
    wait_for_launch_input(&runtime).await;
    wait_for_text(&runtime, "FIXTURE_PROVIDER_RUNNING").await;
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_all_providers_keep_arguments_and_bound_return_on_resume() {
    use std::os::unix::fs::PermissionsExt;
    for kind in ["claude", "codex", "kimi", "cursor", "grok"] {
        for resume in [false, true] {
            let dir = crate::utils::test_temp::unique_temp_path("launch-neighbours");
            std::fs::create_dir_all(&dir).unwrap();
            install_fake_provider(&dir);
            let agent = crate::agents::parse_agent_label(kind).unwrap();
            let executable = crate::agents::interactive_agent_executable(agent);
            let path = dir.join("bin").join(executable);
            std::fs::write(&path, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/arguments\"\nprintf 'FIXTURE_PROVIDER_RUNNING\\n'\nread -r unused\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(dir.join(".bashrc"), "PS1='READY> '\n").unwrap();
            let runtime = fixture_shell(
                "/bin/bash",
                crate::utils::config::ShellModeConfig::NonLogin,
                &dir,
            );
            wait_for_prompt(&runtime).await;
            let (mut app, target) = app_with_prompt_runtime(runtime);
            let args = if resume {
                vec!["--resume".into(), "saved-session".into()]
            } else {
                vec![]
            };
            app.start_agent(crate::protocol::api::schema::AgentStartParams {
                name: "neighbour".into(),
                kind: kind.into(),
                pane_id: target.clone(),
                args: args.clone(),
                timeout_ms: None,
                executable: matches!(kind, "grok" | "kimi").then(|| path.display().to_string()),
                grok_home: (kind == "grok").then(|| dir.display().to_string()),
                kimi_home: (kind == "kimi").then(|| dir.display().to_string()),
            })
            .unwrap_or_else(|error| {
                panic!(
                    "{kind} resume={resume}: {:?}",
                    app.agent_start_error_body(error)
                )
            });
            wait_for_launch_input(prompt_runtime(&app)).await;
            wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
            app.handle_agent_list("live-poll".into());
            let info = app.agent_info_for_target(&target).unwrap();
            assert_eq!(
                info.name.as_deref(),
                Some("neighbour"),
                "{kind} resume={resume}"
            );
            assert!(!prompt_runtime(&app).launch_returned_to_prompt());
            if resume {
                assert!(std::fs::read_to_string(dir.join("arguments"))
                    .unwrap()
                    .starts_with("--resume\nsaved-session\n"));
            }
            // The provider really exits; a matching next primary prompt must
            // release startup ownership even before the first process probe.
            prompt_runtime(&app)
                .try_send_bytes(Bytes::from_static(b"\r"))
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !prompt_runtime(&app).launch_returned_to_prompt() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{kind}: {}",
                    prompt_runtime(&app).shell_prompt_diagnostics()
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            app.handle_agent_list("exit-poll".into());
            assert!(
                app.agent_info_for_target(&target).is_err(),
                "{kind} exit retained ownership"
            );
            drop(app);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[tokio::test]
async fn verified_launch_missing_command_is_a_real_return() {
    let dir = crate::utils::test_temp::unique_temp_path("launch-missing-command");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".bashrc"), "PS1='READY> '\n").unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let (mut app, target) = app_with_prompt_runtime(runtime);
    try_prompt_launch(&mut app, &target).unwrap();
    wait_for_launch_input(prompt_runtime(&app)).await;
    wait_for_text(prompt_runtime(&app), "command not found").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !prompt_runtime(&app).launch_returned_to_prompt() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    app.handle_agent_list("missing-poll".into());
    assert!(app.agent_info_for_target(&target).is_err());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_prompt_cycles_preserve_user_ps1_appends() {
    let dir = crate::utils::test_temp::unique_temp_path("launch-ps1-appends");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(
        dir.join(".bashrc"),
        "PS1='READY> '; PROMPT_COMMAND=\"PS1+='tail>'\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("bin/codex"),
        "#!/bin/sh\nprintf 'PROVIDER_EXIT\\n'\nexit 1\n",
    )
    .unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    runtime
        .queue_shell_command(&["codex".into()], LAUNCH_SIGNAL_TIMEOUT)
        .unwrap();
    wait_for_launch_input(&runtime).await;
    wait_for_text(&runtime, "PROVIDER_EXIT").await;
    wait_for_signal(
        || runtime.launch_returned_to_prompt(),
        || runtime.shell_prompt_diagnostics(),
    )
    .await;
    wait_for_signal(
        || runtime.visible_text().contains("READY> tail>tail>"),
        || runtime.visible_text(),
    )
    .await;
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn verified_launch_late_old_prompt_after_enter_is_not_a_return() {
    let dir = crate::utils::test_temp::unique_temp_path("launch-late-redraw");
    std::fs::create_dir_all(&dir).unwrap();
    install_fake_provider(&dir);
    std::fs::write(dir.join(".bashrc"), "PS1='READY> '\n").unwrap();
    std::fs::write(dir.join("bin/codex"), "#!/bin/sh\nprintf started > \"$HOME/started\"\nprintf 'FIXTURE_PROVIDER_RUNNING\\n'\nread -r unused\n").unwrap();
    let runtime = fixture_shell(
        "/bin/bash",
        crate::utils::config::ShellModeConfig::NonLogin,
        &dir,
    );
    wait_for_prompt(&runtime).await;
    let old_prompt = runtime.test_shell_prompt_marker();
    let barrier = runtime.test_hold_launch_input(true);
    let pid = runtime.child_pid().unwrap();
    struct Release {
        barrier: std::sync::Arc<crate::terminal::runtime::LaunchTestBarrier>,
        pid: u32,
        process: Option<crate::platform::ProcessInstance>,
    }
    impl Drop for Release {
        fn drop(&mut self) {
            if self.process.is_some() && crate::platform::process_instance(self.pid) == self.process
            {
                unsafe {
                    libc::kill(self.pid as libc::pid_t, libc::SIGCONT);
                }
            }
            self.barrier.release();
        }
    }
    let release = Release {
        barrier: barrier.clone(),
        pid,
        process: crate::platform::process_instance(pid),
    };
    let (mut app, target) = app_with_prompt_runtime(runtime);
    try_prompt_launch(&mut app, &target).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !barrier.verification_reached() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) }, 0);
    // Confirm the shell is stopped before allowing the writer to queue Enter.
    loop {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&output.stdout).contains('T') {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    barrier.release_verification();
    while !barrier.reached() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!dir.join("started").exists());
    prompt_runtime(&app).test_process_pty_bytes(&old_prompt);
    app.handle_agent_list("enter-in-flight".into());
    assert_eq!(
        app.agent_info_for_target(&target).unwrap().name.as_deref(),
        Some("fallback")
    );
    // The transaction can finish while Enter is still unread by the stopped
    // shell. Replaying the old prompt *then* must still not count as return.
    barrier.release();
    wait_for_launch_input(prompt_runtime(&app)).await;
    prompt_runtime(&app).test_process_pty_bytes(&old_prompt);
    app.handle_agent_list("submitted-old-prompt".into());
    assert!(app.agent_info_for_target(&target).unwrap().launch_pending);
    assert!(!dir.join("started").exists());
    drop(release);
    wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_launch_aliases_keep_literal_arguments_and_attempt_return() {
    use std::os::unix::fs::PermissionsExt;
    for shell in ["/bin/bash", "/bin/zsh"] {
        for resume in [false, true] {
            let dir = crate::utils::test_temp::unique_temp_path("launch-alias");
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            let provider = dir.join("alias provider");
            std::fs::write(&provider, "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$HOME/alias-argv\"\nprintf 'FIXTURE_ALIAS_RUNNING\\n'\nread -r unused\nexit 23\n").unwrap();
            std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
            // An alias is the sole provider route; the private PATH has no codex.
            let (rc, prompt) = if shell.ends_with("bash") {
                (".bashrc", "RC=$? READY> ")
            } else {
                (".zshrc", "RC=%? READY> ")
            };
            std::fs::write(dir.join(rc), format!("export PATH=\"$HOME/bin\"\nalias codex=\"'{}' --alias-argument\"\nPS1='{prompt}'\n", provider.display())).unwrap();
            let runtime =
                fixture_shell(shell, crate::utils::config::ShellModeConfig::NonLogin, &dir);
            wait_for_prompt(&runtime).await;
            let (mut app, target) = app_with_prompt_runtime(runtime);
            let mut args = if resume {
                vec!["--resume".to_owned(), "saved-session".to_owned()]
            } else {
                vec![]
            };
            args.extend(
                [
                    "one two",
                    "quote'and;$()",
                    "中文",
                    "",
                    "$(touch injected)",
                    ";touch injected",
                ]
                .map(str::to_owned),
            );
            app.start_agent(crate::protocol::api::schema::AgentStartParams {
                name: "alias".into(),
                kind: "codex".into(),
                pane_id: target.clone(),
                args: args.clone(),
                timeout_ms: None,
                executable: None,
                grok_home: None,
                kimi_home: None,
            })
            .unwrap_or_else(|error| {
                panic!(
                    "{shell} resume={resume}: {:?}",
                    app.agent_start_error_body(error)
                )
            });
            wait_for_launch_input(prompt_runtime(&app)).await;
            wait_for_text(prompt_runtime(&app), "FIXTURE_ALIAS_RUNNING").await;
            let actual = std::fs::read_to_string(dir.join("alias-argv")).unwrap();
            let mut expected = vec!["--alias-argument"];
            expected.extend(args.iter().map(String::as_str));
            let actual = actual.split_terminator('\0').collect::<Vec<_>>();
            // Bus appends its provider flags after the caller's arguments.
            // Preserve the complete explicit argument prefix, including alias args.
            assert!(
                actual.starts_with(&expected),
                "{shell} resume={resume}: actual={actual:?}, expected prefix={expected:?}"
            );
            assert!(!dir.join("injected").exists());
            assert!(!prompt_runtime(&app).launch_returned_to_prompt());
            assert_eq!(
                app.agent_info_for_target(&target).unwrap().name.as_deref(),
                Some("alias")
            );
            prompt_runtime(&app)
                .try_send_bytes(Bytes::from_static(b"\r"))
                .unwrap();
            wait_for_text(prompt_runtime(&app), "RC=23 READY>").await;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !prompt_runtime(&app).launch_returned_to_prompt() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{}",
                    prompt_runtime(&app).shell_prompt_diagnostics()
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            app.handle_agent_list("alias-exit".into());
            assert!(app.agent_info_for_target(&target).is_err());
            drop(app);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
