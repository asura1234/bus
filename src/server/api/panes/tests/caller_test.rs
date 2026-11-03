use super::*;

fn locate(app: &mut App, process: crate::platform::ProcessInstance) -> serde_json::Value {
    let request = serde_json::from_value(serde_json::json!({
        "id":"locate-caller", "method":"pane.locate_process",
        "params":{"pid":process.pid,"birth":process.birth}
    }))
    .expect("native caller lookup must be available");
    serde_json::from_str(&app.handle_api_request(request)).unwrap()
}

#[test]
fn sender_identity_native_lookup_rejects_stale_process_incarnations() {
    let (mut app, _) = app_with_test_workspace();
    let me = crate::platform::process_instance(std::process::id()).unwrap();
    let response = locate(
        &mut app,
        crate::platform::ProcessInstance {
            birth: me.birth.wrapping_add(1),
            ..me
        },
    );
    assert_eq!(response["error"]["code"], "caller_unknown");
}

#[test]
fn sender_identity_native_lookup_recognizes_an_external_process() {
    let (mut app, _) = app_with_test_workspace();
    let me = crate::platform::process_instance(std::process::id()).unwrap();
    let response = locate(&mut app, me);
    assert_eq!(
        response["result"]["type"],
        "caller_location",
        "{response}; ancestry={:?}",
        crate::platform::process_ancestry(me.pid)
    );
    assert!(
        response["result"]["location"]["pane"].is_null(),
        "{response}"
    );
    assert_eq!(response["result"]["location"]["agent_owned"], false);
}

#[cfg(unix)]
#[tokio::test]
async fn sender_identity_native_lookup_maps_a_shell_and_descendant_to_its_owned_pane() {
    let (mut app, public) = app_with_test_workspace();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal = app.state.terminal_id_for_pane(0, pane).unwrap();
    let (events, _rx) = tokio::sync::mpsc::channel(64);
    let runtime = crate::terminal::TerminalRuntime::spawn_argv_command(
        pane,
        24,
        80,
        std::env::temp_dir(),
        &["/bin/sh".into(), "-c".into(), "sleep 30 & wait".into()],
        &crate::terminal::runtime::PaneLaunchEnv::default(),
        crate::terminal::runtime::AgentDetection::Disabled,
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let shell = runtime.child_process_instance().unwrap().pid;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let child = loop {
        if let Some(child) = crate::platform::session_processes(shell)
            .into_iter()
            .find(|pid| *pid != shell)
        {
            break child;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "shell child did not start"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    app.terminal_runtimes.insert(terminal.clone(), runtime);
    let shell = crate::platform::process_instance(shell).unwrap();
    let child = crate::platform::process_instance(child).unwrap();
    for process in [shell, child] {
        let response = locate(&mut app, process);
        assert_eq!(
            response["result"]["location"]["pane"]["pane_id"], public,
            "{response}"
        );
        assert_eq!(response["result"]["location"]["agent_owned"], false);
    }
    app.state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .begin_managed_agent(
            "bus-r1-a2".into(),
            AgentKind::Codex,
            std::time::Instant::now(),
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(10),
        );
    let response = locate(&mut app, child);
    assert_eq!(
        response["result"]["location"]["agent_owned"], true,
        "{response}"
    );
    crate::server::api::test_support::shutdown_test_runtimes(&mut app);
}

#[tokio::test]
async fn sender_identity_native_lookup_does_not_guess_around_an_unverifiable_pane_root() {
    let (mut app, _) = app_with_test_workspace();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal = app.state.terminal_id_for_pane(0, pane).unwrap();
    let (runtime, _rx) = crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 1);
    app.terminal_runtimes.insert(terminal, runtime);
    let response = locate(
        &mut app,
        crate::platform::process_instance(std::process::id()).unwrap(),
    );
    assert_eq!(response["error"]["code"], "caller_unknown", "{response}");
}

#[cfg(unix)]
#[tokio::test]
async fn sender_identity_native_lookup_refuses_an_orphan_still_in_a_pane_session() {
    let (mut app, _) = app_with_test_workspace();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal = app.state.terminal_id_for_pane(0, pane).unwrap();
    let (events, _rx) = tokio::sync::mpsc::channel(64);
    let runtime = crate::terminal::TerminalRuntime::spawn_argv_command(
        pane,
        24,
        80,
        std::env::temp_dir(),
        &[
            "/bin/sh".into(),
            "-c".into(),
            "trap '' HUP; (sleep 30 &) ; sleep 30".into(),
        ],
        &crate::terminal::runtime::PaneLaunchEnv::default(),
        crate::terminal::runtime::AgentDetection::Disabled,
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let root = runtime.child_process_instance().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let orphan = loop {
        if let Some(pid) = crate::platform::session_processes(root.pid)
            .into_iter()
            .find(|pid| !crate::platform::process_ancestry(*pid).contains(&root))
        {
            break crate::platform::process_instance(pid).unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "orphan did not start");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    app.terminal_runtimes.insert(terminal, runtime);
    let response = locate(&mut app, orphan);
    crate::server::api::test_support::shutdown_test_runtimes(&mut app);
    assert_eq!(
        response["error"]["code"], "caller_unknown",
        "orphan still owns this pane's PTY session: {response}"
    );
}
