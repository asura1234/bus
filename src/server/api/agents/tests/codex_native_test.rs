use super::*;
use crate::agents::providers::codex::native::test_support;

#[tokio::test]
async fn codex_native_admission_keeps_the_developer_draft_and_writes_no_terminal_bytes() {
    let (mut app, method, mut writes) =
        provider_fixture(AgentKind::Codex, true, "developer draft 保留");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    let before = runtime.visible_text();
    let (path, server) = test_support::server_for("session", false, false);
    *runtime.codex_native().lock().unwrap() = test_support::connected_runtime(path);
    let response = start(&mut app, method);
    let value = response
        .recv_timeout(Duration::from_millis(500))
        .expect("native admission must bypass composer preservation");
    assert!(value.contains("agent_prompted"), "{value}");
    assert!(
        writes.try_recv().is_err(),
        "native input must never edit the developer's composer"
    );
    assert_eq!(
        app.lookup_runtime_sender(0, pane).unwrap().visible_text(),
        before
    );
    let calls = server.join().unwrap();
    assert_eq!(calls.last().unwrap()["method"], "thread/queue/add");
}

#[tokio::test]
async fn codex_native_lost_ack_does_not_fall_back_to_typing() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (path, server) = test_support::server_for("session", false, true);
    *app.lookup_runtime_sender(0, pane)
        .unwrap()
        .codex_native()
        .lock()
        .unwrap() = test_support::connected_runtime(path);
    let response = start(&mut app, method);
    let value = response
        .recv_timeout(Duration::from_millis(500))
        .expect("uncertain native result must settle without a typed retry");
    assert!(value.contains("agent_prompt_failed"), "{value}");
    assert!(writes.try_recv().is_err());
    let calls = server.join().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["method"] == "thread/queue/add")
            .count(),
        1
    );
}

#[tokio::test]
async fn codex_native_missing_socket_uses_the_existing_guarded_typed_fallback() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let dir = crate::utils::test_temp::unique_temp_path("codex-native-missing");
    std::fs::create_dir_all(&dir).unwrap();
    *app.lookup_runtime_sender(0, pane)
        .unwrap()
        .codex_native()
        .lock()
        .unwrap() = test_support::connected_runtime(dir.join("missing"));
    let response = start(&mut app, method);
    accept_paste(&app, AgentKind::Codex, "from bus", &mut writes).await;
    let value = response.recv_timeout(WAIT).unwrap();
    assert!(value.contains("agent_prompted"), "{value}");
}

#[tokio::test]
async fn codex_native_does_not_intercept_a_literal_escape_key_request() {
    let (mut app, _, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let info = app.agent_info(0, pane).unwrap();
    let (path, server) = test_support::server_for("session", false, false);
    let native = app.lookup_runtime_sender(0, pane).unwrap().codex_native();
    *native.lock().unwrap() = test_support::connected_runtime(path);
    let params =
        serde_json::from_value(serde_json::json!({"target":info.pane_id,"keys":["esc"]})).unwrap();
    let response = app.handle_agent_send_keys("literal-key".into(), params);
    let sent = writes.try_recv().ok();
    // Drain the fixture even when the request correctly avoided native RPC.
    let _ = native.lock().unwrap().interrupt(
        "session",
        std::time::Instant::now() + Duration::from_secs(3),
    );
    server.join().unwrap();
    assert!(response.contains("\"type\":\"ok\""), "{response}");
    assert_eq!(
        sent.as_deref(),
        Some(&b"\x1b"[..]),
        "literal Esc must reach the TUI rather than become an idle native no-op"
    );
}

#[tokio::test]
async fn codex_native_interrupt_rejects_a_stale_session_without_pressing_escape() {
    let (mut app, _, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let info = app.agent_info(0, pane).unwrap();
    let (path, server) = test_support::server_for("session", false, false);
    let native = app.lookup_runtime_sender(0, pane).unwrap().codex_native();
    *native.lock().unwrap() = test_support::connected_runtime(path);
    let params = serde_json::from_value(serde_json::json!({
        "target":info.pane_id,"keys":["esc"],
        "native_interrupt":{"expected_terminal_id":info.terminal_id,"expected_session_id":"retired-session"}
    })).unwrap();
    let response = app.handle_agent_send_keys("stale-stop".into(), params);
    assert!(writes.try_recv().is_err());
    let _ = native.lock().unwrap().interrupt(
        "session",
        std::time::Instant::now() + Duration::from_secs(3),
    );
    let calls = server.join().unwrap();
    assert!(response.contains("agent_not_ready"), "{response}");
    assert!(!calls.iter().any(|call| call["method"] == "turn/interrupt"));
}

fn show_working_projection(app: &mut App) {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    assert_eq!(
        app.agent_info(0, pane).unwrap().agent_status,
        crate::protocol::api::schema::AgentStatus::Working
    );
}

#[tokio::test]
async fn codex_native_idle_admits_despite_a_transient_working_projection() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "developer draft");
    show_working_projection(&mut app);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let native = app.lookup_runtime_sender(0, pane).unwrap().codex_native();
    let (path, server) = test_support::server_for("session", false, false);
    *native.lock().unwrap() = test_support::connected_runtime(path);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    // On the red implementation, drain the untouched external fixture.
    if !response.contains("agent_prompted") {
        let _ = native.lock().unwrap().interrupt(
            "session",
            std::time::Instant::now() + Duration::from_secs(3),
        );
    }
    let calls = server.join().unwrap();
    assert!(response.contains("agent_prompted"), "{response}");
    assert!(writes.try_recv().is_err());
    assert_eq!(calls.last().unwrap()["method"], "thread/queue/add");
}

#[tokio::test]
async fn codex_native_actual_busy_turn_refuses_unsteered_working_projection() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    show_working_projection(&mut app);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let native = app.lookup_runtime_sender(0, pane).unwrap().codex_native();
    let (path, server) = test_support::server_for("session", true, false);
    *native.lock().unwrap() = test_support::connected_runtime(path);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    let _ = native.lock().unwrap().interrupt(
        "session",
        std::time::Instant::now() + Duration::from_secs(3),
    );
    let calls = server.join().unwrap();
    assert!(
        response.contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN),
        "{response}"
    );
    assert!(writes.try_recv().is_err());
    assert!(!calls
        .iter()
        .any(|call| call["method"] == "thread/queue/add" || call["method"] == "turn/steer"));
}

#[tokio::test]
async fn codex_native_working_projection_never_uses_unavailable_channel_fallback() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    show_working_projection(&mut app);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_not_idle"), "{response}");
    assert!(writes.try_recv().is_err());
}
