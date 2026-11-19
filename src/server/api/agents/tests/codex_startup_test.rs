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
        let terminal_id = app.agent_info(0, pane).unwrap().terminal_id;
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
