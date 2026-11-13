use super::*;

#[test]
fn headless_prompt_dispatch_preserves_the_socket_receipt_budget() {
    let mut server = test_headless_server();
    let (respond_to, response) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
        received_at: Instant::now() - Duration::from_secs(4),
        request: crate::protocol::api::schema::Request {
            id: "expired_before_lookup".into(),
            method: crate::protocol::api::schema::Method::AgentPrompt(
                crate::protocol::api::schema::AgentPromptParams {
                    target: "not-looked-up".into(),
                    text: "from bus".into(),
                    wait: None,
                },
            ),
        },
        respond_to,
    });
    let response: serde_json::Value = serde_json::from_str(&response.recv().unwrap()).unwrap();
    assert_eq!(
        response["error"]["code"],
        crate::protocol::api::AGENT_PROMPT_NOT_SHOWN
    );
    assert_eq!(server.app.terminal_runtimes.len(), 0);
}

#[test]
fn client_launched_server_saves_and_stops_after_last_client_grace() {
    let mut server = test_headless_server();
    server.stop_without_clients = true;
    let (writer, _control_rx, _render_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new((80, 24), Default::default(), 1, writer),
    );
    let now = Instant::now();
    assert!(!server.check_client_lifetime(now));
    server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 1 });
    assert!(!server.check_client_lifetime(now));
    assert!(!server.check_client_lifetime(now + Duration::from_millis(2999)));
    assert!(server.check_client_lifetime(now + Duration::from_secs(3)));
    assert!(server.shutting_down);
    assert!(server.app.state.should_quit);
}

#[test]
fn remaining_or_reattached_client_cancels_shutdown_grace() {
    let mut server = test_headless_server();
    server.stop_without_clients = true;
    let now = Instant::now();
    assert!(!server.check_client_lifetime(now));
    for id in 1..=2 {
        let (writer, _control_rx, _render_rx) = test_client_writer();
        server.clients.insert(
            id,
            ClientConnection::new((80, 24), Default::default(), id, writer),
        );
    }
    assert!(!server.check_client_lifetime(now + Duration::from_secs(4)));
    server.remove_client(1);
    assert!(!server.check_client_lifetime(now + Duration::from_secs(8)));
    server.remove_client(2);
    assert!(!server.check_client_lifetime(now + Duration::from_secs(9)));
    assert!(server.check_client_lifetime(now + Duration::from_secs(12)));
}

#[test]
fn explicit_headless_server_survives_clientless_periods() {
    let mut server = test_headless_server();
    let now = Instant::now();
    assert!(!server.check_client_lifetime(now));
    assert!(!server.check_client_lifetime(now + Duration::from_secs(60)));
    assert!(!server.shutting_down);
}

#[test]
fn default_headless_size_is_effective_without_clients() {
    let server = test_headless_server();

    assert_eq!(
        server.headless_size,
        (
            crate::utils::config::DEFAULT_HEADLESS_COLS,
            crate::utils::config::DEFAULT_HEADLESS_ROWS
        )
    );
    assert_eq!(server.effective_size, server.headless_size);
}

#[test]
fn server_stop_interrupts_server_event_backlog() {
    let mut server = test_headless_server();
    for client_id in 1..=64 {
        server
            .server_event_tx
            .try_send(ServerEvent::ClientDisconnected { client_id })
            .unwrap();
    }

    server.should_quit.store(true, Ordering::Release);

    assert!(!server.drain_server_events());
    assert!(server.server_event_rx.try_recv().is_ok());
    shutdown_test_runtimes(&mut server);
}

#[test]
fn headless_api_request_drains_all_pending_internal_events_before_reading_state() {
    let mut server = test_headless_server();
    for _ in 0..=crate::server::app::APP_EVENT_DRAIN_LIMIT {
        server
            .app
            .event_tx
            .try_send(TerminalEvent::TerminalCwdReported {
                pane_id: crate::utils::ids::PaneId::from_raw(9_999),
                cwd: "relative".into(),
            })
            .unwrap();
    }

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    assert!(
        server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
            received_at: std::time::Instant::now(),
            request: crate::protocol::api::schema::Request {
                id: "headless_stop_after_events".into(),
                method: crate::protocol::api::schema::Method::ServerStop(
                    crate::protocol::api::schema::EmptyParams::default()
                ),
            },
            respond_to,
        })
    );
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "ok");
    assert!(server.app.event_rx.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn headless_scheduled_tasks_start_pending_agent_resume_without_foreground_client() {
    let mut server = test_headless_server();
    let workspace = crate::server::workspaces::Workspace::test_new("restored");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.ensure_test_terminals();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });

    server.render_and_stream();
    assert_ne!(server.app.state.view.terminal_area, Rect::default());

    let now = Instant::now();
    assert!(!server.handle_scheduled_tasks_headless(now, false));
    assert!(server.app.terminal_runtimes.get(&terminal_id).is_none());
    let deadline = server
        .app
        .pending_agent_resume_deadline
        .expect("clientless resume should wait briefly for a host theme");

    assert!(server.handle_scheduled_tasks_headless(deadline, false));
    assert!(server.app.terminal_runtimes.get(&terminal_id).is_some());
    assert!(server
        .app
        .state
        .terminals
        .get(&terminal_id)
        .expect("test terminal should still exist")
        .pending_agent_resume_plan
        .is_none());
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
fn restored_native_server_for_restart() -> (HeadlessServer, crate::utils::ids::TerminalId) {
    let mut server = test_headless_server();
    let cwd = std::env::current_dir().unwrap();
    let snapshot = serde_json::from_value(serde_json::json!({
        "version": 3, "active": 0, "selected": 0,
        "workspaces": [{ "id": "restored", "identity_cwd": cwd,
            "active_tab": 0, "tabs": [{ "layout": { "Pane": 10 },
                "panes": { "10": { "cwd": cwd, "label": null,
                    "agent_name": "worker", "managed_agent_kind": "codex",
                    "agent_session": { "source": "herdr:codex", "agent": "codex",
                        "kind": "id", "value": "saved-session" } } },
                "zoomed": false, "focused": 10, "root_pane": 10 }] }]
    }))
    .unwrap();
    let restored = crate::server::persistence::restore(
        &snapshot,
        None,
        true,
        server.app.event_tx.clone(),
        server.app.render_notify.clone(),
        server.app.render_dirty.clone(),
    );
    server.app.state.workspaces = restored.workspaces;
    server.app.state.terminals = restored.terminals;
    server.app.state.active = Some(0);
    let terminal_id = server.app.state.terminals.keys().next().unwrap().clone();
    // A benign native process: never launch an installed provider from a test.
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .pending_agent_resume_plan
        .as_mut()
        .unwrap()
        .argv = vec!["/bin/sh".into(), "-c".into(), "read line".into()];
    server.app.state.default_shell = "/bin/bash".into();
    server.render_and_stream();
    (server, terminal_id)
}

/// A real restore plan, then the same rejected API input that arrives from the
/// coordinator immediately after restart. Virtual time isolates the deadline
/// reset from provider startup speed and the coordinator's retry interval.
#[cfg(unix)]
#[tokio::test]
async fn rejected_post_restart_prompt_does_not_reset_native_resume_deadline() {
    let (mut server, terminal_id) = restored_native_server_for_restart();
    let pane_id = server.app.agent_info_for_target("worker").unwrap().pane_id;
    let now = Instant::now();
    assert!(!server.handle_scheduled_tasks_headless(now, false));
    let deadline = server.app.pending_agent_resume_deadline.unwrap();

    for offset in [Duration::from_millis(500), Duration::from_millis(1000)] {
        let (respond_to, response) = std::sync::mpsc::channel();
        let changed =
            server.handle_api_request_with_shutdown_check(crate::server::api::ApiRequestMessage {
                received_at: Instant::now(),
                request: api::schema::Request {
                    id: "early-send".into(),
                    method: api::schema::Method::AgentPromptIfIdle(
                        api::schema::AgentPromptIfIdleParams {
                            target: "worker".into(),
                            text: "immediate".into(),
                            expected_terminal_id: terminal_id.to_string(),
                            expected_pane_id: pane_id.clone(),
                            expected_agent: "codex".into(),
                            expected_session_id: "saved-session".into(),
                            steer: false,
                            expected_generation: None,
                            expected_turn_id: None,
                        },
                    ),
                },
                respond_to,
            });
        let reply: serde_json::Value = serde_json::from_str(&response.recv().unwrap()).unwrap();
        assert_eq!(reply["error"]["code"], "agent_not_ready");
        assert!(changed, "the rejected prompt still requests a repaint");
        server.handle_scheduled_tasks_headless(now + offset, changed);
        assert_eq!(
            server.app.pending_agent_resume_deadline,
            Some(deadline),
            "a repaint must not restart the native resume theme wait"
        );
        server.render_and_stream();
    }
    assert!(server.handle_scheduled_tasks_headless(deadline, false));
    assert!(server.app.terminal_runtimes.get(&terminal_id).is_some());
    shutdown_test_runtimes(&mut server);
}

#[test]
fn drained_api_requests_are_timed_like_waited_ones() {
    let mut server = test_headless_server();
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    server.app.api_rx = api_rx;
    let _ = crate::server::perf::API.take();
    let mut responses = Vec::new();
    for id in ["queued-1", "queued-2"] {
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        api_tx
            .send(crate::server::api::ApiRequestMessage {
                received_at: std::time::Instant::now(),
                request: api::schema::Request {
                    id: id.into(),
                    method: api::schema::Method::PaneList(api::schema::PaneListParams::default()),
                },
                respond_to,
            })
            .unwrap();
        responses.push(response_rx);
    }

    server.drain_api_requests_with_shutdown_check();
    server.drain_api_requests_with_shutdown_check();

    for response in responses {
        response.try_recv().expect("drained request answered");
    }
    let window = crate::server::perf::API
        .take()
        .expect("drained requests are recorded in server.api");
    assert_eq!(window.count, 2, "{window:?}");
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn post_render_native_resume_progresses_during_repaints_with_or_without_theme() {
    for (theme_ready, full_render) in [(false, true), (true, true), (false, false)] {
        let (mut server, terminal_id) = restored_native_server_for_restart();
        let saved = server.app.agent_info_for_target("worker").unwrap();
        assert!(
            saved.launch_pending,
            "the saved provider still needs to resume"
        );
        assert!(
            !saved.interactive_ready,
            "saved metadata is not readiness evidence"
        );
        assert!(
            !saved.native_ready,
            "there is no recreated PTY/provider yet"
        );
        if theme_ready {
            server.app.state.host_terminal_theme.foreground =
                Some(crate::utils::theme::color::RgbColor {
                    r: 220,
                    g: 220,
                    b: 220,
                });
        }
        let now = Instant::now();
        let (mut needs_render, mut needs_full_render) = (true, full_render);
        assert!(server.render_pending_loop_work(now, &mut needs_render, &mut needs_full_render));
        if !theme_ready {
            assert!(server.app.terminal_runtimes.get(&terminal_id).is_none());
            let deadline = server.app.pending_agent_resume_deadline.unwrap();
            for offset in [Duration::from_millis(400), Duration::from_millis(800)] {
                // Every iteration has repaint work, including when the theme
                // wait expires; a clean/idle scheduler pass is never supplied.
                needs_render = true;
                needs_full_render = full_render;
                server.handle_scheduled_tasks_headless(now + offset, true);
                assert_eq!(server.app.pending_agent_resume_deadline, Some(deadline));
                assert!(server.render_pending_loop_work(
                    now + offset,
                    &mut needs_render,
                    &mut needs_full_render,
                ));
            }
        }
        assert!(server.app.terminal_runtimes.get(&terminal_id).is_some());
        assert!(
            needs_render && needs_full_render,
            "publish the newly started runtime"
        );
        assert!(server.app.pending_agent_resume_deadline.is_none());
        // A shell runtime alone is not a provider-readiness signal either.
        assert!(
            !server
                .app
                .agent_info_for_target("worker")
                .unwrap()
                .native_ready
        );
        shutdown_test_runtimes(&mut server);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn render_without_pending_resume_does_not_derive_resume_geometry() {
    let (mut server, terminal_id) = restored_native_server_for_restart();
    let plan = server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .pending_agent_resume_plan
        .take();
    assert!(plan.is_some());
    let now = Instant::now();
    server.app.pending_agent_resume_deadline = Some(now);
    let (mut needs_render, mut needs_full_render) = (true, true);
    assert!(server.render_pending_loop_work(now, &mut needs_render, &mut needs_full_render));
    assert_eq!(
        server
            .app
            .resume_geometry_derivations
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a normal repaint must not derive resume geometry without a pending plan"
    );
    assert!(!needs_render && !needs_full_render);
    assert!(server.app.pending_agent_resume_deadline.is_none());

    // Positive control: the observer records real layout work when a pending
    // plan exists, rather than asserting only a helper's unchanged false return.
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .pending_agent_resume_plan = plan;
    needs_render = true;
    needs_full_render = true;
    assert!(server.render_pending_loop_work(
        now + Duration::from_secs(1),
        &mut needs_render,
        &mut needs_full_render,
    ));
    assert!(
        server
            .app
            .resume_geometry_derivations
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn headless_native_interrupt_handshake_does_not_block_api_dispatch() {
    use crate::agents::{AgentKind, AgentState};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    const WAIT: Duration = crate::terminal::runtime::NATIVE_TEST_TIMEOUT;
    let mut server = test_headless_server();
    let workspace = crate::server::workspaces::Workspace::test_new("native-interrupt");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().clone();
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.ensure_test_terminals();
    let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        AgentKind::Codex,
        now,
        Duration::ZERO,
        Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
    terminal.set_agent_session_ref_for_session_start(
        "bus".into(),
        "codex".into(),
        crate::agents::resume::catalog::AgentSessionRef::id("session"),
        Some(1),
        None,
        &[],
    );
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes(b"\x1b[?2004h\x1b[2J\x1b[H> draft\r\n\r\n GPT-6 /repo");
    server.app.state.insert_test_runtime(pane, runtime);
    let info = server.app.agent_info(0, pane).unwrap();
    let dir = crate::utils::test_temp::unique_temp_path("codex-interrupt-dispatch");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let (returned, dispatcher_returned) = mpsc::channel();
    let native_peer = std::thread::spawn(move || {
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
        server.app.lookup_runtime_sender(0, pane).unwrap(),
        path,
    );
    let params: crate::protocol::api::schema::AgentSendKeysParams = serde_json::from_value(serde_json::json!({
        "target":info.pane_id,"keys":["esc"],
        "native_interrupt":{"expected_terminal_id":info.terminal_id,"expected_session_id":"session"}
    })).unwrap();
    let (send, response) = mpsc::channel();
    // Exercise the real server API routing, not the App helper in isolation.
    server.handle_timed_api_request(crate::server::api::ApiRequestMessage {
        received_at: Instant::now(),
        request: crate::protocol::api::schema::Request {
            id: "headless-interrupt-dispatch".into(),
            method: crate::protocol::api::schema::Method::AgentSendKeys(params),
        },
        respond_to: send,
    });
    returned.send(()).unwrap();
    let value = response.recv_timeout(WAIT).unwrap();
    let interrupted = native_peer.join().unwrap();
    assert!(
        interrupted && value.contains("\"type\":\"agent_interrupted\""),
        "interrupt must complete after dispatch returned: {value}"
    );
    assert!(
        writes.try_recv().is_err(),
        "a native interrupt must not press Esc"
    );
}

// Runs the production drain/schedule/render path with the coordinator's poll.
// No providers, live Bus state, terminal IO threads, or machine-specific load.
#[tokio::test]
async fn idle_agent_poll_server_loop_p99_under_50ms() {
    use crate::agents::{AgentKind, AgentState};
    use crate::protocol::api::schema::{
        AgentDetectionContext, AgentDetectionContexts, Method, Request,
    };
    let mut server = test_headless_server();
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    server.app.api_rx = api_rx;
    server.app.state.workspaces = (0..20)
        .map(|i| crate::server::workspaces::Workspace::test_new(&format!("idle-{i}")))
        .collect();
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    let screen = include_str!("../api/agents/tests/fixtures/claude_2_1_310_idle_88cols.txt");
    let mut contexts = Vec::new();
    let mut receivers = Vec::new();
    for ws in 0..20 {
        let pane = server.app.state.workspaces[ws].tabs[0].root_pane;
        let terminal_id = server.app.state.workspaces[ws]
            .terminal_id(pane)
            .unwrap()
            .clone();
        let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
        terminal.set_agent_name(format!("idle-{ws}"));
        let (runtime, writes) = crate::terminal::TerminalRuntime::test_with_channel(180, 65);
        runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
        receivers.push(writes);
        server.app.state.insert_test_runtime(pane, runtime);
        contexts.push(AgentDetectionContext {
            pane_id: server.app.public_pane_id(ws, pane).unwrap(),
            terminal_id: terminal_id.to_string(),
            session_id: None,
            obligation: false,
            activity_age_ms: None,
        });
    }
    // Settle initial geometry before publishing the detector's idle snapshots.
    server.render_and_stream();
    for ws in 0..20 {
        let pane = server.app.state.workspaces[ws].tabs[0].root_pane;
        server
            .app
            .lookup_runtime_sender(ws, pane)
            .unwrap()
            .test_scan_detection_context(AgentKind::Claude, AgentState::Unknown)
            .await;
    }
    let mut samples = Vec::new();
    for sample in 0..45 {
        let (respond_to, response) = std::sync::mpsc::channel();
        api_tx
            .send(crate::server::api::ApiRequestMessage {
                received_at: Instant::now(),
                request: Request {
                    id: sample.to_string(),
                    method: Method::AgentListWithDetectionContext(AgentDetectionContexts {
                        agents: contexts.clone(),
                    }),
                },
                respond_to,
            })
            .unwrap();
        let started = Instant::now();
        server.test_loop_work();
        let elapsed = started.elapsed();
        let reply: crate::protocol::api::schema::SuccessResponse =
            serde_json::from_str(&response.try_recv().unwrap()).unwrap();
        let crate::protocol::api::schema::ResponseResult::AgentList { agents } = reply.result
        else {
            panic!("agent list");
        };
        assert_eq!(agents.len(), 20);
        if sample >= 5 {
            assert!(
                agents.iter().all(|agent| !agent.screen_detection_skipped),
                "the budget must measure current detector publications for all 20 panes"
            );
            samples.push(elapsed);
        }
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    eprintln!(
        "server.loop 20 idle agent panes: median={:?} p99={p99:?}",
        samples[samples.len() / 2]
    );
    assert!(
        p99 < Duration::from_millis(50),
        "server.loop p99={p99:?} must be <50ms"
    );
    assert!(receivers.iter_mut().all(|rx| rx.try_recv().is_err()));
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn pane_input_runs_before_queued_api_and_api_backlog_yields() {
    use crate::protocol::api::schema::{Method, PaneSendTextParams, Request};
    let mut server = test_headless_server();
    let mut input = install_focused_test_runtime(&mut server, b"");
    let pane_id = server.app.session_snapshot().focused_pane_id.unwrap();
    server.clients.insert(
        11,
        ClientConnection::new((80, 24), Default::default(), 1, unread_test_writer()),
    );
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    server.app.api_rx = api_rx;
    let mut replies = Vec::new();
    for i in 0..3 {
        let (respond_to, reply) = std::sync::mpsc::channel();
        replies.push(reply);
        api_tx
            .send(crate::server::api::ApiRequestMessage {
                received_at: Instant::now(),
                request: Request {
                    id: i.to_string(),
                    method: Method::PaneSendText(PaneSendTextParams {
                        pane_id: pane_id.clone(),
                        text: "api".into(),
                    }),
                },
                respond_to,
            })
            .unwrap();
    }
    server
        .server_event_tx
        .try_send(ServerEvent::ClientShellPaneInput {
            client_id: 11,
            pane_id,
            events: vec![crate::protocol::wire::ClientPaneInputEvent::TextCommit(
                "key".into(),
            )],
        })
        .unwrap();
    server.test_loop_work();
    assert_eq!(
        input.try_recv().unwrap(),
        Bytes::from_static(b"key"),
        "pane input takes priority over API work already queued"
    );
    assert_eq!(input.try_recv().unwrap(), Bytes::from_static(b"api"));
    assert_eq!(
        server.app.api_rx.len(),
        2,
        "a backlog must yield to input/render between requests"
    );
    for _ in 0..2 {
        server.test_loop_work();
    }
    assert!(
        server.app.api_rx.is_empty(),
        "API work still makes progress"
    );
    assert!(replies.iter().all(|rx| rx.try_recv().is_ok()));
    shutdown_test_runtimes(&mut server);
}
