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
    crate::server::terminals::native_input::test_bind(runtime, path);
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
    assert_eq!(calls.last().unwrap()["method"], "thread/queue/start");
}

#[tokio::test]
async fn codex_native_lost_ack_does_not_fall_back_to_typing() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (path, server) = test_support::server_for("session", false, true);
    crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
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
async fn codex_native_disconnected_remote_refuses_without_terminal_bytes() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let dir = crate::utils::test_temp::unique_temp_path("codex-native-missing");
    std::fs::create_dir_all(&dir).unwrap();
    crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        dir.join("missing"),
    );
    let response = start(&mut app, method);
    let value = response.recv_timeout(WAIT).unwrap();
    assert!(value.contains("agent_prompt_not_shown"), "{value}");
    assert!(
        writes.try_recv().is_err(),
        "an attached remote pane has no standalone backend"
    );
}

#[tokio::test]
async fn codex_native_does_not_intercept_a_literal_escape_key_request() {
    let (mut app, _, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let info = app.agent_info(0, pane).unwrap();
    let (path, server) = test_support::server_for("session", false, false);
    let native = crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let params =
        serde_json::from_value(serde_json::json!({"target":info.pane_id,"keys":["esc"]})).unwrap();
    let response = app.handle_agent_send_keys("literal-key".into(), params);
    let sent = writes.try_recv().ok();
    // Drain the fixture even when the request correctly avoided native RPC.
    let _ = native.interrupt(
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
    let native = crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let params = serde_json::from_value(serde_json::json!({
        "target":info.pane_id,"keys":["esc"],
        "native_interrupt":{"expected_terminal_id":info.terminal_id,"expected_session_id":"retired-session"}
    })).unwrap();
    let response = app.handle_agent_send_keys("stale-stop".into(), params);
    assert!(writes.try_recv().is_err());
    let _ = native.interrupt(
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
    let (path, server) = test_support::server_for("session", false, false);
    let native = crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    // On the red implementation, drain the untouched external fixture.
    if !response.contains("agent_prompted") {
        let _ = native.interrupt(
            "session",
            std::time::Instant::now() + Duration::from_secs(3),
        );
    }
    let calls = server.join().unwrap();
    assert!(response.contains("agent_prompted"), "{response}");
    assert!(writes.try_recv().is_err());
    assert_eq!(calls.last().unwrap()["method"], "thread/queue/start");
}

#[tokio::test]
async fn codex_native_actual_busy_turn_refuses_unsteered_working_projection() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    show_working_projection(&mut app);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (path, server) = test_support::server_for("session", true, false);
    let native = crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    let _ = native.interrupt(
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

#[tokio::test]
async fn codex_native_handshake_cannot_hold_the_app_dispatcher() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::Instant;

    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    // Native-only admission also avoids starting a typed writer on a red run.
    show_working_projection(&mut app);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let dir = crate::utils::test_temp::unique_temp_path("codex-dispatch-handshake");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let (returned, dispatcher_returned) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.set_write_timeout(Some(WAIT)).unwrap();
        // Causal barrier, not a timing assertion: the provider cannot finish
        // its handshake until App dispatch has returned to its caller.
        dispatcher_returned.recv_timeout(WAIT).unwrap();
        let Ok(mut ws) = tungstenite::accept(stream) else {
            return false;
        };
        loop {
            let Ok(frame) = ws.read() else { return false };
            let value: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            let Some(id) = value.get("id") else { continue };
            let method = value["method"].as_str().unwrap();
            let result = match method {
                "initialize" => serde_json::json!({"userAgent":"fixture"}),
                "thread/read" => serde_json::json!({"thread":{
                    "id":"session", "status":{"type":"idle"}, "turns":[]
                }}),
                "thread/queue/add" => {
                    serde_json::json!({"queuedSubmission":{"id":"handshake-queue","clientUserMessageId":value["params"]["clientUserMessageId"]}})
                }
                "thread/queue/start" => {
                    serde_json::json!({"turn":{"id":"handshake-turn","status":"inProgress"}})
                }
                other => panic!("unexpected native operation {other}"),
            };
            if ws
                .send(tungstenite::Message::Text(
                    serde_json::json!({"id":id,"result":result})
                        .to_string()
                        .into(),
                ))
                .is_err()
            {
                return false;
            }
            if method == "thread/queue/start" {
                return true;
            }
        }
    });
    crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let budget = crate::protocol::api::submission::method_budget(&method).unwrap();
    let (respond_to, response) = mpsc::channel();
    let handled = app.handle_deferred_agent_api_request_at(
        Request {
            id: "native-handshake-dispatch".into(),
            method,
        },
        respond_to,
        Instant::now() - budget + Duration::from_secs(3),
    );
    // An asynchronous native writer permits this signal while it is still
    // connecting. Synchronous availability cannot: its read must time out.
    returned.send(()).unwrap();
    let value = response.recv_timeout(WAIT).unwrap();
    let admitted_after_dispatch_returned = server.join().unwrap();
    assert!(handled);
    assert!(
        writes.try_recv().is_err(),
        "native-only refusal must not type"
    );
    assert!(
        admitted_after_dispatch_returned && value.contains("agent_prompted"),
        "App dispatch must return before awaiting the native handshake: {value}"
    );
}

#[tokio::test]
async fn codex_native_preflight_approval_is_definite_non_delivery() {
    use std::os::unix::net::UnixListener;
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let dir = crate::utils::test_temp::unique_temp_path("codex-preflight-approval");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.set_write_timeout(Some(WAIT)).unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let mut calls = Vec::new();
        while let Ok(frame) = ws.read() {
            let value: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            calls.push(value.clone());
            let Some(id) = value.get("id") else { continue };
            let result = match value["method"].as_str().unwrap() {
                "initialize" => serde_json::json!({"userAgent":"fixture"}),
                "thread/read" => serde_json::json!({"thread":{
                    "id":"session", "status":{"type":"active","activeFlags":["waitingOnApproval"]},
                    "turns":[{"id":"approval-turn","status":"inProgress","items":[]}]
                }}),
                other => panic!("preflight refusal must not mutate: {other}"),
            };
            ws.send(tungstenite::Message::Text(
                serde_json::json!({"id":id,"result":result})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        }
        calls
    });
    crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    // The UI guard was idle; the authoritative native read now sees approval.
    let value = start(&mut app, method).recv_timeout(WAIT).unwrap();
    let calls = server.join().unwrap();
    assert!(calls.iter().any(|call| call["method"] == "thread/read"));
    assert!(!calls.iter().any(|call| matches!(
        call["method"].as_str(),
        Some("thread/queue/add" | "turn/steer")
    )));
    assert!(
        writes.try_recv().is_err(),
        "a native approval must not trigger typed input"
    );
    assert!(
        value.contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN),
        "preflight saw approval before any mutation; retain definite non-delivery: {value}"
    );
}

#[tokio::test]
async fn codex_native_interrupt_handshake_cannot_hold_the_app_dispatcher() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    let (mut app, _, mut writes) = provider_fixture(AgentKind::Codex, true, "draft");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let info = app.agent_info(0, pane).unwrap();
    let dir = crate::utils::test_temp::unique_temp_path("codex-interrupt-dispatch");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let (returned, dispatcher_returned) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        dispatcher_returned.recv_timeout(WAIT).unwrap();
        let Ok(mut ws) = tungstenite::accept(stream) else {
            return false;
        };
        let mut interrupted = false;
        let mut enumerated = false;
        while let Ok(frame) = ws.read() {
            let value: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            let Some(id) = value.get("id") else { continue };
            let method = value["method"].as_str().unwrap();
            let result = match method {
                "initialize" => serde_json::json!({"userAgent":"fixture"}),
                "thread/read" => serde_json::json!({"thread":{
                    "id":"session", "status":if interrupted {serde_json::json!({"type":"idle"})} else {serde_json::json!({"type":"active","activeFlags":[]})},
                    "turns":[{"id":"interrupt-turn","status":"inProgress","items":[]}]
                }}),
                "thread/turns/list" => {
                    serde_json::json!({"data":[{"id":"interrupt-turn","status":if interrupted {"interrupted"} else {"inProgress"},"items":[]}],"nextCursor":null})
                }
                "turn/interrupt" => {
                    interrupted = true;
                    serde_json::json!({})
                }
                "thread/backgroundTerminals/list" => {
                    enumerated = true;
                    serde_json::json!({"data":[],"nextCursor":null})
                }
                other => panic!("unexpected interrupt operation {other}"),
            };
            if ws
                .send(tungstenite::Message::Text(
                    serde_json::json!({"id":id,"result":result})
                        .to_string()
                        .into(),
                ))
                .is_err()
            {
                return false;
            }
            if enumerated && method == "thread/turns/list" {
                return true;
            }
        }
        false
    });
    crate::server::terminals::native_input::test_bind(
        app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let params: crate::protocol::api::schema::AgentSendKeysParams = serde_json::from_value(serde_json::json!({
        "target":info.pane_id,"keys":["esc"],
        "native_interrupt":{"expected_terminal_id":info.terminal_id,"expected_session_id":"session"}
    })).unwrap();
    let (send, response) = mpsc::channel();
    let handled = app.handle_deferred_agent_api_request(
        Request {
            id: "interrupt-dispatch".into(),
            method: Method::AgentSendKeys(params.clone()),
        },
        send.clone(),
    );
    // This is the production server's immediate path when deferral declines.
    if !handled {
        send.send(app.handle_agent_send_keys("interrupt-dispatch".into(), params))
            .unwrap();
    }
    returned.send(()).unwrap();
    let value = response.recv_timeout(WAIT).unwrap();
    let interrupted = server.join().unwrap();
    assert!(
        handled && interrupted && value.contains("\"type\":\"agent_interrupted\""),
        "interrupt must complete after dispatch returned: {value}"
    );
    assert!(
        writes.try_recv().is_err(),
        "a native interrupt must not press Esc"
    );
}

mod draft_admission {
    use super::*;

    async fn project_stale_blocked_draft(app: &mut App) {
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let runtime = app.lookup_runtime_sender(0, pane).unwrap();
        runtime.set_detection_context(
            AgentKind::Codex,
            true,
            None,
            crate::server::api::agents::prompt::detection_composer(AgentKind::Codex),
        );
        let (state, _) = runtime
            .test_scan_detection_context(AgentKind::Codex, AgentState::Idle)
            .await;
        assert_eq!(
            state,
            AgentState::Idle,
            "a preservable draft remains ready under the current detection policy"
        );
        // Admission must also tolerate a stale Blocked projection when the
        // fresh active screen proves that only a developer draft is present.
        let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        app.state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
        let info = app.agent_info(0, pane).unwrap();
        assert_eq!(
            info.agent_status,
            crate::protocol::api::schema::AgentStatus::Blocked
        );
        assert!(info.dialog_id.is_none());
        assert!(info.interactive_ready);
    }

    #[tokio::test]
    async fn codex_native_draft_projection_uses_native_turn_state_without_touching_the_draft() {
        for (active, steer) in [(false, false), (true, true), (true, false)] {
            let (mut app, mut method, mut writes) =
                provider_fixture(AgentKind::Codex, true, "developer draft 保留");
            project_stale_blocked_draft(&mut app).await;
            if let Method::AgentPromptIfIdle(params) = &mut method {
                params.steer = steer;
                params.expected_turn_id = steer.then(|| "busy-turn".into());
            }
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            let runtime = app.lookup_runtime_sender(0, pane).unwrap();
            let before = runtime.visible_text();
            let (path, server) = test_support::server_for("session", active, false);
            let native = crate::server::terminals::native_input::test_bind(runtime, path);
            let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
            if !response.contains("agent_prompted") {
                // Drain the server on RED as well as on authoritative busy refusal.
                let _ = native.interrupt(
                    "session",
                    std::time::Instant::now() + Duration::from_secs(3),
                );
            }
            let calls = server.join().unwrap();
            assert!(
                writes.try_recv().is_err(),
                "native-only admission never types"
            );
            assert_eq!(
                app.lookup_runtime_sender(0, pane).unwrap().visible_text(),
                before
            );
            if !active || steer {
                assert!(
                    response.contains("agent_prompted"),
                    "{active}/{steer}: {response}"
                );
                assert_eq!(
                    calls.last().unwrap()["method"],
                    if steer {
                        "turn/steer"
                    } else {
                        "thread/queue/start"
                    }
                );
            } else {
                assert!(
                    response.contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN),
                    "{response}"
                );
                assert!(!calls.iter().any(|call| matches!(
                    call["method"].as_str(),
                    Some("thread/queue/add" | "thread/queue/start" | "turn/steer")
                )));
            }
        }
    }

    #[tokio::test]
    async fn codex_native_draft_projection_has_no_unavailable_channel_typed_fallback() {
        let (mut app, method, mut writes) =
            provider_fixture(AgentKind::Codex, true, "developer draft 保留");
        project_stale_blocked_draft(&mut app).await;
        let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
        assert!(response.contains("agent_not_idle"), "{response}");
        assert!(writes.try_recv().is_err());
    }

    #[tokio::test]
    async fn codex_native_draft_projection_does_not_bypass_other_blockers() {
        for screen in [
            "Provider unavailable; sign in again",
            "New version available\r\n❯ Install update now\r\n  Continue with current version\r\n↑/↓ to select, Enter to confirm",
            "›\r\n GPT-6 · /repo\r\n ? for shortcuts",
            "\x1b[?2026h› developer draft\r\n GPT-6 · /repo\r\n ? for shortcuts",
        ] {
            let (mut app, method, mut writes) =
                provider_fixture(AgentKind::Codex, true, "developer draft 保留");
            project_stale_blocked_draft(&mut app).await;
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            let runtime = app.lookup_runtime_sender(0, pane).unwrap();
            runtime.test_process_pty_bytes(format!("\x1b[2J\x1b[H{screen}").as_bytes());
            let (path, server) = test_support::server_for("session", false, false);
            let native = crate::server::terminals::native_input::test_bind(runtime, path);
            let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
            let _ = native.interrupt(
                "session",
                std::time::Instant::now() + Duration::from_secs(3),
            );
            let calls = server.join().unwrap();
            assert!(!response.contains("agent_prompted"), "{screen}: {response}");
            assert!(writes.try_recv().is_err());
            assert!(!calls.iter().any(|call| matches!(
                call["method"].as_str(),
                Some("thread/queue/add" | "thread/queue/start" | "turn/steer")
            )));
        }
    }

    #[tokio::test]
    async fn codex_native_draft_projection_reads_the_active_screen_when_scrolled() {
        for draft in [true, false] {
            let (mut app, method, _) =
                provider_fixture(AgentKind::Codex, true, "developer draft 保留");
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            let (runtime, mut writes) =
                crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                    100, 24, 100_000, b"", 16,
                );
            runtime.test_process_pty_bytes(
                screen(AgentKind::Codex, "developer draft 保留").as_bytes(),
            );
            app.state.insert_test_runtime(pane, runtime);
            project_stale_blocked_draft(&mut app).await;
            let runtime = app.lookup_runtime_sender(0, pane).unwrap();
            let live = if draft {
                "› current draft 保留\n GPT-6 · /repo\n ? for shortcuts"
            } else {
                "Provider unavailable; sign in again"
            };
            let text = format!(
                "\x1b[2J\x1b[H› historical draft\n GPT-6 · /repo\n ? for shortcuts\n{}{live}",
                "captured transcript\n".repeat(60)
            );
            runtime.test_process_pty_bytes(text.replace('\n', "\r\n").as_bytes());
            runtime.set_scroll_offset_from_bottom(60);
            let before = runtime.visible_text();
            assert!(before.contains("historical draft"), "{before}");
            assert!(!before.contains("current draft"));
            let (path, server) = test_support::server_for("session", false, false);
            let native = crate::server::terminals::native_input::test_bind(runtime, path);
            let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
            if !response.contains("agent_prompted") {
                let _ = native.interrupt(
                    "session",
                    std::time::Instant::now() + Duration::from_secs(3),
                );
            }
            let calls = server.join().unwrap();
            assert_eq!(response.contains("agent_prompted"), draft, "{response}");
            assert_eq!(
                calls
                    .iter()
                    .any(|call| call["method"] == "thread/queue/add"),
                draft
            );
            assert!(writes.try_recv().is_err());
            assert_eq!(
                app.lookup_runtime_sender(0, pane).unwrap().visible_text(),
                before
            );
        }
    }
}

mod interrupt_completion_fact {
    use super::*;
    use crate::terminal::runtime::native_input::{
        NativeInput, NativeInterruptCompletion, NativePrompt,
    };

    struct CompletedPort(Option<bool>);
    impl NativeInput for CompletedPort {
        fn has_session(&self) -> bool {
            self.0.is_some()
        }
        fn prompt(&self, _: NativePrompt<'_>) -> Option<std::io::Result<()>> {
            None
        }
        fn interrupt(
            &self,
            _: &str,
            _: std::time::Instant,
        ) -> Option<std::io::Result<NativeInterruptCompletion>> {
            self.0.map(|confirmed| {
                if confirmed {
                    Ok(NativeInterruptCompletion {
                        turn_id: Some("turn-1".into()),
                    })
                } else {
                    Err(std::io::Error::other("turn-changed-during-interrupt"))
                }
            })
        }
    }

    #[tokio::test]
    async fn native_interrupt_completion_fact_survives_a_blocked_draft() {
        for native_completed in [None, Some(false), Some(true)] {
            let (mut app, _, mut writes) =
                provider_fixture(AgentKind::Codex, true, "developer draft 保留");
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
            app.state
                .terminals
                .get_mut(&terminal)
                .unwrap()
                .set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
            let info = app.agent_info(0, pane).unwrap();
            let runtime = app.lookup_runtime_sender(0, pane).unwrap();
            let before = runtime.visible_text();
            runtime.bind_native_input(std::sync::Arc::new(CompletedPort(native_completed)));
            let params = serde_json::from_value(serde_json::json!({
                "target":info.pane_id,"keys":["esc"],"native_interrupt":{
                    "expected_terminal_id":info.terminal_id,"expected_session_id":"session"
                }
            }))
            .unwrap();
            let (send, response) = std::sync::mpsc::channel();
            assert!(app.handle_deferred_agent_api_request(
                Request {
                    id: "native-completion".into(),
                    method: Method::AgentSendKeys(params)
                },
                send
            ));
            let text = response.recv_timeout(WAIT).unwrap();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                app.lookup_runtime_sender(0, pane).unwrap().visible_text(),
                before
            );
            if native_completed == Some(true) {
                assert!(writes.try_recv().is_err());
                assert_eq!(value["result"]["type"], "agent_interrupted", "{text}");
                assert_eq!(value["result"]["terminal_id"], info.terminal_id);
                assert_eq!(value["result"]["session_id"], "session");
                assert_eq!(value["result"]["turn_id"], "turn-1");
            } else if native_completed == Some(false) {
                assert_eq!(
                    value["error"]["code"], "agent_interrupt_unconfirmed",
                    "{text}"
                );
                assert!(
                    value["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("turn-changed-during-interrupt")),
                    "{text}"
                );
                assert!(writes.try_recv().is_err());
            } else {
                assert_eq!(
                    value["result"]["type"], "ok",
                    "typed admission is not native completion"
                );
                assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b");
            }
        }
    }
}
