use super::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

// Real pane-owned runtime and native wire protocol; no provider process or model.
fn cold_runtime(app: &App) -> std::path::PathBuf {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    runtime.retain_shared_codex_from_env(&[
        ("BUS_CODEX_NATIVE".into(), "1".into()),
        ("BUS_CALLBACK_DIR".into(), "/unused-capture".into()),
    ]);
    let argv = runtime.launch_shared_codex(vec!["codex".into()]).unwrap();
    std::path::PathBuf::from(&argv[2]).join("s")
}

struct StartupServer {
    state: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    calls: Arc<Mutex<Vec<Value>>>,
    task: Option<std::thread::JoinHandle<()>>,
}

impl StartupServer {
    fn new(path: &std::path::Path) -> Self {
        let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (mode, done, recorded) = (state.clone(), stop.clone(), calls.clone());
        let task = std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                let (stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let Ok(mut ws) = tungstenite::accept(stream) else {
                    continue;
                };
                while let Ok(frame) = ws.read() {
                    let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    recorded.lock().unwrap().push(value.clone());
                    let Some(id) = value.get("id") else { continue };
                    let result = match value["method"].as_str().unwrap() {
                        "initialize" => json!({}),
                        "thread/loaded/list" => match mode.load(Ordering::Acquire) {
                            0 => json!({"data":[],"nextCursor":null}),
                            1 => json!({"data":["session","other"],"nextCursor":null}),
                            4 => json!({"data":["other"],"nextCursor":null}),
                            _ => json!({"data":["session"],"nextCursor":null}),
                        },
                        "thread/read" => json!({"thread":{"id":"session","status":
                            if mode.load(Ordering::Acquire) == 2 {
                                json!({"type":"active","activeFlags":[]})
                            } else { json!({"type":"idle"}) }
                        }}),
                        "thread/turns/list" => {
                            json!({"data":[{"id":"real-turn","status":"inProgress"}],"nextCursor":null})
                        }
                        "thread/queue/add" => json!({"queuedSubmission":{"id":"queued"}}),
                        "thread/queue/start" => json!({}),
                        other => panic!("unexpected native operation: {other}"),
                    };
                    if ws
                        .send(tungstenite::Message::Text(
                            json!({"id":id,"result":result}).to_string().into(),
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            state,
            stop,
            calls,
            task: Some(task),
        }
    }
}

impl Drop for StartupServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.task.take().unwrap().join().unwrap();
    }
}

#[tokio::test]
async fn codex_startup_waits_for_native_thread_before_reporting_ready_or_submitting() {
    for bound in [false, true] {
        let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, bound, "");
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let path = cold_runtime(&app);
        let shared = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .shared_codex_handle();
        let held = shared.lock().unwrap();
        assert!(
            !app.agent_info(0, pane).unwrap().interactive_ready,
            "a contended native input lock is not startup readiness"
        );
        drop(held);
        let info = app.agent_info(0, pane).unwrap();
        assert!(
            !info.interactive_ready,
            "screen Idle before native startup is false readiness: {info:?}"
        );
        assert!(info.launch_pending);
        assert!(!info.native_ready);
        let server = StartupServer::new(&path);
        // Empty, ambiguous and actually active threads are not startup readiness.
        for mode in 0..3 {
            server.state.store(mode, Ordering::Release);
            for _ in 0..5 {
                assert!(!app.agent_info(0, pane).unwrap().interactive_ready);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(!server
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|v| v["method"] == "thread/queue/add"));
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
        server.state.store(3, Ordering::Release);
        // The second observed notice came from startup Working chrome, before
        // any model turn. Native idle alone must not open that input window.
        for _ in 0..5 {
            let info = app.agent_info(0, pane).unwrap();
            assert_eq!(
                info.agent_status,
                crate::protocol::api::schema::AgentStatus::Working
            );
            assert!(!info.interactive_ready);
            assert!(info.launch_pending);
            std::thread::sleep(Duration::from_millis(20));
        }
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if app.agent_info(0, pane).unwrap().interactive_ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "idle loaded native thread must enable delivery: info={:?}, calls={:?}",
                app.agent_info(0, pane).unwrap(),
                server.calls.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let info = app.agent_info(0, pane).unwrap();
        assert!(!info.launch_pending);
        let held = shared.lock().unwrap();
        assert!(
            app.agent_info(0, pane).unwrap().interactive_ready,
            "an admitted native turn must retain its startup proof during contention"
        );
        drop(held);
        let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
        assert!(response.contains("agent_prompted"), "{response}");
        assert!(
            writes.try_recv().is_err(),
            "native submission must not type"
        );
        // Re-add/relaunch on the same pane must not inherit the previous readiness.
        cold_runtime(&app);
        assert!(!app.agent_info(0, pane).unwrap().interactive_ready);
    }
}

#[tokio::test]
async fn codex_startup_waits_for_the_bound_thread_identity() {
    let (app, _, _) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let server = StartupServer::new(&cold_runtime(&app));
    server.state.store(4, Ordering::Release);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let info = app.agent_info(0, pane).unwrap();
        assert!(info.launch_pending && !info.interactive_ready && !info.native_ready);
        let calls = server.calls.lock().unwrap();
        assert!(!calls.iter().any(|call| call["method"] == "thread/read"));
        // A second list call proves the first foreign result settled and was retried.
        if calls
            .iter()
            .filter(|call| call["method"] == "thread/loaded/list")
            .count()
            >= 2
        {
            break;
        }
        drop(calls);
        assert!(
            std::time::Instant::now() < deadline,
            "foreign thread probe did not retry"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    server.state.store(3, Ordering::Release);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let info = app.agent_info(0, pane).unwrap();
        if info.interactive_ready {
            assert!(!info.launch_pending && info.native_ready);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "bound thread never became ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
async fn codex_startup_snapshot_reports_the_native_readiness_gate() {
    let (app, _, _) = provider_fixture(AgentKind::Codex, false, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    cold_runtime(&app);
    let info = app.agent_info(0, pane).unwrap();
    let response = app.handle_agent_startup_snapshot(
        "diagnostics".into(),
        crate::protocol::api::schema::AgentStartupSnapshotParams {
            pane_id: info.pane_id,
            expected_terminal_id: info.terminal_id,
        },
    );
    let value: Value = serde_json::from_str(&response).unwrap();
    let snapshot = &value["result"]["snapshot"];
    assert_eq!(snapshot["launch_pending"], true, "{response}");
    assert_eq!(snapshot["interactive_ready"], false, "{response}");
}

#[tokio::test]
async fn claude_startup_readiness_does_not_wait_for_codex_native_metadata() {
    let (app, _, _) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    cold_runtime(&app);
    let info = app.agent_info(0, pane).unwrap();
    assert!(info.interactive_ready);
    assert!(!info.launch_pending);
}

#[tokio::test]
async fn codex_standalone_startup_keeps_visual_readiness_without_a_native_endpoint() {
    let (app, _, _) = provider_fixture(AgentKind::Codex, false, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let path = cold_runtime(&app);
    std::fs::write(path.parent().unwrap().join("mode"), "standalone").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !app.agent_info(0, pane).unwrap().interactive_ready {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !path.exists(),
        "standalone readiness must not create a native endpoint"
    );
}

#[tokio::test]
async fn codex_startup_cached_idle_requires_current_screen_evidence() {
    use crate::terminal::runtime::native_input::{
        NativeInput, NativeInterruptCompletion, NativePrompt,
    };
    struct IdleBackend(AtomicBool);
    impl NativeInput for IdleBackend {
        fn has_session(&self) -> bool {
            true
        }
        fn startup_ready(&self, _thread: &str, frontend_idle: bool) -> bool {
            if frontend_idle {
                self.0.store(true, Ordering::Release);
            }
            self.0.load(Ordering::Acquire)
        }
        fn prompt(&self, _: NativePrompt<'_>) -> Option<std::io::Result<()>> {
            None
        }
        fn interrupt(
            &self,
            _: &str,
            _: std::time::Instant,
        ) -> Option<std::io::Result<NativeInterruptCompletion>> {
            None
        }
    }
    fn list(app: &mut App) -> crate::protocol::api::schema::AgentInfo {
        let response: crate::protocol::api::schema::SuccessResponse =
            serde_json::from_str(&app.handle_agent_list("startup-cache".into())).unwrap();
        let crate::protocol::api::schema::ResponseResult::AgentList { mut agents } =
            response.result
        else {
            panic!("agent list")
        };
        agents.remove(0)
    }
    let (mut app, _, _) = provider_fixture(AgentKind::Codex, false, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .bind_native_input(Arc::new(IdleBackend(AtomicBool::new(false))));
    let cold = list(&mut app);
    assert!(cold.screen_detection_skipped);
    assert_eq!(
        cold.agent_status,
        crate::protocol::api::schema::AgentStatus::Unknown
    );
    assert!(
        !cold.interactive_ready,
        "stale Idle cannot confirm startup: {cold:?}"
    );
    assert!(cold.launch_pending);
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_publish_screen_detection(AgentKind::Codex);
    let fresh = list(&mut app);
    assert!(!fresh.screen_detection_skipped);
    assert!(fresh.interactive_ready);
    assert!(!fresh.launch_pending);
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(b"\r\nrepaint");
    let admitted = list(&mut app);
    assert!(admitted.screen_detection_skipped);
    assert!(
        admitted.interactive_ready,
        "an admitted launch retains its readiness through an inconclusive poll"
    );
    assert!(!admitted.launch_pending);
}

fn ready_unbound_native() -> (
    App,
    Method,
    tokio::sync::mpsc::Receiver<Bytes>,
    StartupServer,
) {
    let (app, method, writes) = provider_fixture(AgentKind::Codex, false, "keep developer draft");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let path = cold_runtime(&app);
    let server = StartupServer::new(&path);
    server.state.store(3, Ordering::Release);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !app.agent_info(0, pane).unwrap().interactive_ready {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    (app, method, writes, server)
}

fn project_unbound_working(app: &mut App) {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    let info = app.agent_info(0, pane).unwrap();
    assert!(info.interactive_ready);
    assert!(info.agent_session.is_none());
    assert_eq!(
        info.agent_status,
        crate::protocol::api::schema::AgentStatus::Working
    );
}

#[tokio::test]
async fn codex_native_unbound_working_frontend_uses_idle_native_admission() {
    let (mut app, method, mut writes, server) = ready_unbound_native();
    project_unbound_working(&mut app);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(
        response.contains("agent_prompted"),
        "native idle is usable despite startup Working chrome: {response}"
    );
    assert!(
        writes.try_recv().is_err(),
        "native admission must preserve the draft without typing"
    );
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v["method"] == "thread/queue/add")
            .count(),
        1
    );
}

#[tokio::test]
async fn codex_native_unbound_working_frontend_refuses_real_busy_turn_without_typing() {
    let (mut app, method, mut writes, server) = ready_unbound_native();
    server.state.store(2, Ordering::Release);
    project_unbound_working(&mut app);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(
        response.contains("agent_not_idle")
            || response.contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN),
        "{response}"
    );
    assert!(writes.try_recv().is_err());
    assert!(!server.calls.lock().unwrap().iter().any(|v| matches!(
        v["method"].as_str(),
        Some("thread/queue/add" | "thread/queue/start" | "turn/steer")
    )));
}

#[tokio::test]
async fn codex_native_unbound_working_frontend_has_no_typed_fallback() {
    let (mut app, method, mut writes) =
        provider_fixture(AgentKind::Codex, false, "keep developer draft");
    project_unbound_working(&mut app);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_not_idle"), "{response}");
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_native_unbound_working_frontend_keeps_managed_identity_guard() {
    let (mut app, mut method, mut writes, server) = ready_unbound_native();
    project_unbound_working(&mut app);
    let Method::AgentPromptIfUnbound(params) = &mut method else {
        panic!("unbound prompt")
    };
    params.expected_managed_name = "other launch".into();
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_identity_changed"), "{response}");
    assert!(writes.try_recv().is_err());
    assert!(!server
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|v| v["method"] == "thread/queue/add"));
}
