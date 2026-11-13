use super::*;

// No native detection events are applied by this fixture. The command returns
// before the detector can acquire it; the shell's owned return is the proof.
#[tokio::test]
async fn early_startup_exit_exposes_owned_return_before_process_detection() {
    for shell in ["/bin/bash", "/bin/zsh"] {
        let dir = crate::utils::test_temp::unique_temp_path("early-startup-exit");
        std::fs::create_dir_all(&dir).unwrap();
        install_fake_provider(&dir);
        std::fs::write(dir.join(".bashrc"), "PS1='READY> '\n").unwrap();
        std::fs::write(dir.join(".zshrc"), "PS1='READY> '\n").unwrap();
        std::fs::write(
            dir.join("bin/codex"),
            "#!/bin/sh\nprintf \"error: unexpected argument '--invalid-startup-flag' found\\n\"\nexit 2\n",
        ).unwrap();
        let runtime = fixture_shell(shell, crate::utils::config::ShellModeConfig::NonLogin, &dir);
        wait_for_prompt(&runtime).await;
        let (mut app, target) = app_with_prompt_runtime(runtime);
        try_prompt_launch(&mut app, &target).unwrap();
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        assert!(!app.pane_info(0, pane).unwrap().agent_process_exited);
        wait_for_launch_input(prompt_runtime(&app)).await;
        wait_for_signal(
            || prompt_runtime(&app).launch_returned_to_prompt(),
            || prompt_runtime(&app).shell_prompt_diagnostics(),
        )
        .await;
        assert!(app.state.terminals[&terminal]
            .effective_known_agent()
            .is_none());
        app.handle_agent_list("exit-poll".into());
        let exited = app.pane_info(0, pane).unwrap().agent_process_exited;
        let snapshot = app.handle_agent_startup_snapshot(
            "exit-screen".into(),
            crate::protocol::api::schema::AgentStartupSnapshotParams {
                pane_id: target.clone(),
                expected_terminal_id: terminal.to_string(),
            },
        );
        let snapshot: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        assert!(snapshot["result"]["snapshot"]["screen"]
            .as_str()
            .unwrap()
            .contains("error: unexpected argument '--invalid-startup-flag' found"));
        assert!(app.agent_info_for_target(&target).is_err());
        assert!(
            exited,
            "{shell}: witnessed return lost the startup exit signal"
        );
        // A new owned launch must retire the previous exit before it can be
        // mistaken for another immediate failure.
        install_fake_provider(&dir);
        try_prompt_launch(&mut app, &target).unwrap();
        assert!(!app.pane_info(0, pane).unwrap().agent_process_exited);
        wait_for_text(prompt_runtime(&app), "FIXTURE_PROVIDER_RUNNING").await;
        app.handle_agent_list("live-poll".into());
        assert!(!app.pane_info(0, pane).unwrap().agent_process_exited);
        assert!(app.agent_info_for_target(&target).is_ok());
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
