//! Shared Codex launch wiring through the server-owned pane boundary.

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

#[tokio::test]
async fn codex_native_pane_start_enters_the_shared_runtime_and_pinned_launch_keeps_its_argv() {
    let capture = crate::utils::test_temp::unique_temp_path("codex-shared-launch");
    std::fs::create_dir_all(&capture).unwrap();
    let native = started_pane("shared", None, vec!["--no-daemon".into()], &capture).await;
    let pinned_dir = crate::utils::test_temp::unique_temp_path("codex-pinned-keep");
    std::fs::create_dir_all(&pinned_dir).unwrap();
    let pinned_exe = pinned_dir.join("codex");
    let pinned = started_pane(
        "pinned",
        Some(pinned_exe.to_string_lossy().into_owned()),
        vec![
            "--profile".into(),
            "team".into(),
            "--sandbox".into(),
            "read-only".into(),
        ],
        &capture,
    )
    .await;
    let _ = std::fs::remove_dir_all(&capture);
    let _ = std::fs::remove_dir_all(&pinned_dir);
    assert!(
        native.argv.iter().any(|arg| arg == "--bus-codex-runtime")
            && native.command.contains("--bus-codex-runtime")
            && native.command.contains(capture.to_string_lossy().as_ref())
            && native.command.contains("codex")
            && native.session_open,
        "a native pane start must enter the retained runtime: argv={:?} command={} session={}",
        native.argv,
        native.command,
        native.session_open
    );
    assert!(
        !pinned.argv.iter().any(|arg| arg == "--bus-codex-runtime")
            && !pinned.command.contains("--bus-codex-runtime")
            && pinned.command.contains("--profile")
            && pinned.command.contains("team")
            && pinned.command.contains("--sandbox")
            && pinned.command.contains("read-only")
            && !pinned.session_open,
        "a pinned profile launch must keep its argv: argv={:?} command={} session={}",
        pinned.argv,
        pinned.command,
        pinned.session_open
    );
}

struct StartedPane {
    argv: Vec<String>,
    command: String,
    session_open: bool,
}

async fn started_pane(
    name: &str,
    executable: Option<String>,
    args: Vec<String>,
    capture: &std::path::Path,
) -> StartedPane {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::server::app::App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.default_shell = "/bin/sh".into();
    let workspace = crate::server::workspaces::Workspace::test_new(name);
    let pane = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    let terminal = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 4);
    runtime.retain_shared_codex_from_env(&[
        ("BUS_CODEX_NATIVE".into(), "1".into()),
        (
            "BUS_CALLBACK_DIR".into(),
            capture.to_string_lossy().into_owned(),
        ),
    ]);
    app.terminal_runtimes.insert(terminal.clone(), runtime);
    let params = serde_json::json!({
        "name": name,
        "kind": "codex",
        "pane_id": app.public_pane_id(0, pane).unwrap(),
        "args": args,
        "timeout_ms": 4000,
    });
    let mut params = params;
    if let Some(executable) = executable {
        params["executable"] = serde_json::Value::String(executable);
    }
    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: name.into(),
        method: crate::protocol::api::schema::Method::AgentStart(
            serde_json::from_value(params).unwrap(),
        ),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    let argv = response["result"]["argv"]
        .as_array()
        .map(|argv| {
            argv.iter()
                .filter_map(|arg| arg.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let command = writes
        .try_recv()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let session_open = app
        .terminal_runtimes
        .get(&terminal)
        .is_some_and(|runtime| runtime.shared_codex_session_open());
    assert_eq!(
        response["result"]["type"], "agent_started",
        "pane start failed: {response}"
    );
    StartedPane {
        argv,
        command,
        session_open,
    }
}
