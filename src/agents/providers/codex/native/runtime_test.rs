use super::*;

#[test]
fn codex_native_attached_profile_retains_the_configured_tui_without_starting_a_server() {
    let dir = crate::utils::test_temp::unique_temp_path("codex-profile");
    fs::create_dir_all(&dir).unwrap();
    let mut runtime = Runtime {
        config: Some(Config {
            capture: dir.clone(),
        }),
        session: None,
    };
    let argv = ["/not-a-program/codex", "-pwork", "--no-daemon"].map(str::to_owned);
    let result = runtime.launch(&argv).unwrap();
    let started = dir.join("codex-native.log").exists();
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(result, argv);
    assert!(!started,"a config profile must select the legacy TUI before a differently configured app-server starts");
}

#[test]
fn codex_native_explicit_permission_cli_retains_the_configured_tui_before_a_backend_starts() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::utils::test_temp::unique_temp_path("codex-policy-fallback");
    fs::create_dir_all(&dir).unwrap();
    let program = dir.join("codex");
    let marker = dir.join("backend-started");
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf started > '{}'\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let mut runtime = Runtime {
        config: Some(Config {
            capture: dir.clone(),
        }),
        session: None,
    };
    let mut attempted = Vec::new();
    for flags in [
        vec!["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        vec!["--sandbox=read-only"],
        vec!["--ask-for-approval=on-request"],
        vec!["-sread-only"],
        vec!["-aon-request"],
        vec!["--yolo"],
        vec!["--dangerously-bypass-approvals-and-sandbox"],
    ] {
        let mut argv = vec![program.to_string_lossy().into_owned(), "--no-daemon".into()];
        argv.extend(flags.into_iter().map(str::to_owned));
        argv.extend(["resume".into(), "fixture-thread".into()]);
        assert_eq!(runtime.launch(&argv).unwrap(), argv);
        if marker.exists() {
            attempted.push(argv);
            fs::remove_file(&marker).unwrap();
        }
    }
    drop(runtime);
    fs::remove_dir_all(dir).unwrap();
    assert!(attempted.is_empty(), "incompatible remote permission options must choose the original TUI before starting a native writer: {attempted:?}");
}

#[test]
fn codex_native_fresh_permission_options_select_the_available_native_transport() {
    // Real Codex0.164.0 thread/start witnesses establish both policy contracts.
    for args in [
        vec!["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        vec!["--yolo"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(
            supports_native(&args),
            "fresh permissions support native thread/start: {args:?}"
        );
    }
}

#[test]
fn codex_native_permission_fallback_uses_effective_resume_or_fork_options() {
    for (args, native) in [
        (vec!["--sandbox=read-only", "resume", "thread"], false),
        (vec!["fork", "thread", "--yolo"], false),
        (vec!["--yolo", "-c", "resume"], true),
        (vec!["resume", "thread", "--", "--sandbox=read-only"], true),
        (
            vec!["-c", "note=--sandbox=read-only", "resume", "thread"],
            true,
        ),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(supports_native(&args), native, "{args:?}");
    }
}

#[tokio::test]
async fn codex_native_pinned_profile_and_permission_argv_reach_the_pane_shell() {
    let dir = crate::utils::test_temp::unique_temp_path("codex-pin-launch");
    std::fs::create_dir_all(&dir).unwrap();
    let pinned = crate::agents::providers::launch::selected_executable(
        crate::agents::providers::ProviderKind::Codex,
        &std::collections::HashMap::from([("PATH".into(), dir.to_string_lossy().into_owned())]),
    )
    .unwrap()
    .expect("a PATH pin must select the codex executable");
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::server::app::App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.default_shell = "/bin/sh".into();
    let workspace = crate::server::workspaces::Workspace::test_new("codex-pin");
    let pane = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    let terminal = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 4);
    app.terminal_runtimes.insert(terminal, runtime);
    let params = serde_json::from_value(serde_json::json!({
        "name": "nativeworker",
        "kind": "codex",
        "pane_id": app.public_pane_id(0, pane).unwrap(),
        "args": ["--profile", "team", "--sandbox", "read-only"],
        "executable": pinned,
        "timeout_ms": 4000,
    }))
    .unwrap();
    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "pin".into(),
        method: crate::protocol::api::schema::Method::AgentStart(params),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    let command = writes
        .try_recv()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        response["result"]["type"] == "agent_started"
            && command.as_ref().is_some_and(|command| {
                command.contains("--profile")
                    && command.contains("team")
                    && command.contains("--sandbox")
                    && command.contains("read-only")
            }),
        "profile and permission argv must reach the pane shell: response={response} command={command:?}"
    );
}
