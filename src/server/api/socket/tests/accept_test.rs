use super::connection::{error_response_json, handle_request};
use super::*;
use interprocess::local_socket::traits::Listener as _;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::Mutex;
use tokio::sync::mpsc;

fn env_lock() -> &'static Mutex<()> {
    crate::utils::config::test_config_env_lock()
}

fn unique_test_path(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("herdr-{name}-{}-{nanos}", std::process::id()))
}

fn read_line(stream: &mut LocalStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line
}

fn local_stream_pair(name: &str) -> (LocalStream, LocalStream, PathBuf) {
    let path = unique_test_path(name);
    let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
    let client = crate::platform::ipc::connect_local_stream(&path).unwrap();
    let server = listener.accept().unwrap();
    (client, server, path)
}

fn pane_info(
    pane_id: &str,
    agent_status: crate::protocol::api::schema::AgentStatus,
) -> crate::protocol::api::schema::PaneInfo {
    crate::protocol::api::schema::PaneInfo {
        pane_id: pane_id.into(),
        terminal_id: "term_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        focused: true,
        cwd: None,
        foreground_cwd: None,
        label: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        display_agent: None,
        agent_status,
        state_labels: HashMap::new(),
        tokens: HashMap::new(),
        agent_session: None,
        scroll: None,
        revision: 0,
    }
}

fn spawn_pane_get_responder(
    agent_status: crate::protocol::api::schema::AgentStatus,
) -> (ApiRequestSender, std::thread::JoinHandle<()>) {
    let (api_tx, mut api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
    let responder = std::thread::spawn(move || {
        while let Some(msg) = api_rx.blocking_recv() {
            match msg.request.method {
                Method::PaneGet(_) => msg
                    .respond_to
                    .send(
                        serde_json::to_string(&SuccessResponse {
                            id: msg.request.id,
                            result: ResponseResult::PaneInfo {
                                pane: pane_info("pane_1", agent_status),
                            },
                        })
                        .unwrap(),
                    )
                    .unwrap(),
                Method::EventsWait(_) => msg
                    .respond_to
                    .send(error_response_json(
                        msg.request.id,
                        "unexpected_dispatch",
                        "events.wait should be handled by the api server".into(),
                    ))
                    .unwrap(),
                other => panic!("unexpected request: {other:?}"),
            }
        }
    });
    (api_tx, responder)
}

#[test]
fn socket_path_prefers_explicit_env_override() {
    let _guard = env_lock().lock().unwrap();
    let unique = format!("/tmp/herdr-test-{}.sock", std::process::id());
    std::env::remove_var(crate::utils::paths::SESSION_ENV_VAR);
    crate::utils::paths::clear_explicit_session_for_test();
    std::env::set_var(crate::protocol::api::SOCKET_PATH_ENV_VAR, &unique);
    assert_eq!(socket_path(), PathBuf::from(&unique));
    std::env::remove_var(crate::protocol::api::SOCKET_PATH_ENV_VAR);
}

#[test]
fn socket_path_defaults_to_config_dir_even_when_xdg_runtime_dir_is_set() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::utils::config::test_without_bus_env(&_guard);
    let config_home = unique_test_path("socket-default-config-home");
    let runtime_dir = unique_test_path("socket-default-runtime");
    std::env::remove_var(crate::protocol::api::SOCKET_PATH_ENV_VAR);
    std::env::remove_var(crate::utils::paths::SESSION_ENV_VAR);
    crate::utils::paths::clear_explicit_session_for_test();
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);

    let expected = config_home
        .join(crate::utils::config::app_dir_name())
        .join("herdr.sock");
    assert_eq!(socket_path(), expected);

    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("XDG_RUNTIME_DIR");
}

#[test]
fn socket_path_uses_named_session_dir() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::utils::config::test_without_bus_env(&_guard);
    let config_home = unique_test_path("socket-named-config-home");
    std::env::remove_var(crate::protocol::api::SOCKET_PATH_ENV_VAR);
    crate::utils::paths::clear_explicit_session_for_test();
    std::env::set_var(crate::utils::paths::SESSION_ENV_VAR, "work");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);

    let expected = config_home
        .join(crate::utils::config::app_dir_name())
        .join("sessions")
        .join("work")
        .join("herdr.sock");
    assert_eq!(socket_path(), expected);

    std::env::remove_var(crate::utils::paths::SESSION_ENV_VAR);
    std::env::remove_var("XDG_CONFIG_HOME");
}

#[test]
fn restrict_socket_permissions_sets_user_only_mode() {
    let dir = unique_test_path("socket-perms");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api.sock");
    let _listener = UnixListener::bind(&path).unwrap();

    restrict_socket_permissions(&path).unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, SOCKET_PERMISSION_MODE);

    drop(_listener);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn api_response_outcome_uses_top_level_error_shape() {
    let ok_with_error_text = r#"{"id":"req","result":{"read":{"text":"user said \"error\": \"timeout\"","revision":1}}}"#;
    assert_eq!(api_response_outcome(ok_with_error_text), "ok");

    let timeout =
        r#"{"id":"req","error":{"code":"timeout","message":"timed out waiting for output match"}}"#;
    assert_eq!(api_response_outcome(timeout), "timeout");

    let generic_error = r#"{"id":"req","error":{"code":"server_unavailable","message":"boom"}}"#;
    assert_eq!(api_response_outcome(generic_error), "error");
}

#[test]
fn ping_request_returns_pong() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let response = handle_request(
        Request {
            id: "req_1".into(),
            method: Method::Ping(crate::protocol::api::schema::PingParams::default()),
        },
        &tx,
        default_capabilities(),
        &Arc::new(AtomicBool::new(false)),
    );

    let parsed: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed.id, "req_1");
    assert!(matches!(parsed.result, ResponseResult::Pong { .. }));
}

#[test]
fn server_stop_control_bypasses_app_channel() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let response = handle_request(
        Request {
            id: "priority_stop".into(),
            method: Method::ServerStop(crate::protocol::api::schema::EmptyParams::default()),
        },
        &tx,
        default_capabilities(),
        &stop,
    );

    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], "priority_stop");
    assert_eq!(response["result"]["type"], "ok");
    assert!(stop.load(Ordering::Acquire));

    let rejected = handle_request(
        Request {
            id: "after_stop".into(),
            method: Method::WorkspaceList(crate::protocol::api::schema::EmptyParams::default()),
        },
        &tx,
        default_capabilities(),
        &stop,
    );
    let rejected: serde_json::Value = serde_json::from_str(&rejected).unwrap();
    assert_eq!(rejected["error"]["code"], "server_unavailable");
    assert!(rx.try_recv().is_err());
}

#[test]
fn request_dispatches_to_app_channel() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = Request {
        id: "req_2".into(),
        method: Method::WorkspaceList(crate::protocol::api::schema::EmptyParams::default()),
    };

    let request_for_thread = request.clone();
    let thread = std::thread::spawn(move || {
        handle_request(
            request_for_thread,
            &tx,
            default_capabilities(),
            &Arc::new(AtomicBool::new(false)),
        )
    });

    let msg = rx.blocking_recv().unwrap();
    assert_eq!(msg.request.id, "req_2");
    msg.respond_to
        .send(
            serde_json::to_string(&SuccessResponse {
                id: "req_2".into(),
                result: ResponseResult::Ok {},
            })
            .unwrap(),
        )
        .unwrap();

    let response = thread.join().unwrap();
    let parsed: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed.id, "req_2");
}

#[test]
fn events_wait_agent_status_returns_initial_match() {
    let (api_tx, responder) =
        spawn_pane_get_responder(crate::protocol::api::schema::AgentStatus::Blocked);

    let (mut client, server, _path) = local_stream_pair("api-events-wait-initial");
    client
        .write_all(br#"{"id":"wait_1","method":"events.wait","params":{"match_event":{"event":"pane_agent_status_changed","pane_id":"pane_1","agent_status":"blocked"},"timeout_ms":1000}}"#)
        .unwrap();
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let event_hub = EventHub::default();
    handle_connection(server, &api_tx, &event_hub, &running).unwrap();

    let response: serde_json::Value = serde_json::from_str(&read_line(&mut client)).unwrap();
    assert_eq!(response["id"], "wait_1");
    assert_eq!(response["result"]["type"], "wait_matched");
    assert_eq!(
        response["result"]["event"]["data"]["agent_status"],
        "blocked"
    );
    drop(api_tx);
    responder.join().unwrap();
}

#[test]
fn events_wait_agent_status_times_out_server_side() {
    let (api_tx, responder) =
        spawn_pane_get_responder(crate::protocol::api::schema::AgentStatus::Unknown);

    let (mut client, server, _path) = local_stream_pair("api-events-wait-timeout");
    client
        .write_all(br#"{"id":"wait_2","method":"events.wait","params":{"match_event":{"event":"pane_agent_status_changed","pane_id":"pane_1","agent_status":"blocked"},"timeout_ms":30}}"#)
        .unwrap();
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let event_hub = EventHub::default();
    handle_connection(server, &api_tx, &event_hub, &running).unwrap();

    let response: serde_json::Value = serde_json::from_str(&read_line(&mut client)).unwrap();
    assert_eq!(response["id"], "wait_2");
    assert_eq!(response["error"]["code"], "timeout");
    assert_eq!(
        response["error"]["message"],
        "timed out waiting for event match"
    );
    drop(api_tx);
    responder.join().unwrap();
}

#[test]
fn events_wait_agent_status_returns_not_found_when_pane_closes() {
    let event_hub = EventHub::default();
    let responder_event_hub = event_hub.clone();
    let (api_tx, mut api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
    let responder = std::thread::spawn(move || {
        let mut pane_get_count = 0;
        while let Some(msg) = api_rx.blocking_recv() {
            let Method::PaneGet(_) = msg.request.method else {
                panic!("unexpected request: {:?}", msg.request.method);
            };
            pane_get_count += 1;
            let response = if pane_get_count == 1 {
                serde_json::to_string(&SuccessResponse {
                    id: msg.request.id,
                    result: ResponseResult::PaneInfo {
                        pane: pane_info(
                            "pane_1",
                            crate::protocol::api::schema::AgentStatus::Unknown,
                        ),
                    },
                })
                .unwrap()
            } else {
                if pane_get_count == 2 {
                    responder_event_hub.push(crate::protocol::api::schema::EventEnvelope {
                        event: crate::protocol::api::schema::EventKind::PaneClosed,
                        data: crate::protocol::api::schema::EventData::PaneClosed {
                            pane_id: "pane_1".into(),
                            workspace_id: "ws_1".into(),
                        },
                    });
                }
                error_response_json(
                    msg.request.id,
                    "pane_not_found",
                    "pane pane_1 not found".into(),
                )
            };
            msg.respond_to.send(response).unwrap();
        }
    });

    let (mut client, server, _path) = local_stream_pair("wait-close");
    client
        .write_all(br#"{"id":"wait_close","method":"events.wait","params":{"match_event":{"event":"pane_agent_status_changed","pane_id":"pane_1","agent_status":"done"},"timeout_ms":500}}"#)
        .unwrap();
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let running = Arc::new(AtomicBool::new(true));
    handle_connection(server, &api_tx, &event_hub, &running).unwrap();

    let response: serde_json::Value = serde_json::from_str(&read_line(&mut client)).unwrap();
    assert_eq!(response["id"], "wait_close");
    assert_eq!(response["error"]["code"], "pane_not_found");
    assert_eq!(response["error"]["message"], "pane pane_1 not found");
    drop(api_tx);
    responder.join().unwrap();
}

#[test]
fn wait_for_output_stops_when_client_disconnects() {
    let (api_tx, mut api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
    let (first_read_tx, first_read_rx) = std::sync::mpsc::channel();
    let responder = std::thread::spawn(move || {
        let mut notified = false;
        while let Some(msg) = api_rx.blocking_recv() {
            assert!(matches!(msg.request.method, Method::PaneRead(_)));
            if !notified {
                first_read_tx.send(()).unwrap();
                notified = true;
            }
            msg.respond_to
                .send(
                    serde_json::to_string(&SuccessResponse {
                        id: msg.request.id,
                        result: ResponseResult::PaneRead {
                            read: crate::protocol::api::schema::PaneReadResult {
                                pane_id: "pane_1".into(),
                                workspace_id: "ws_1".into(),
                                tab_id: "tab_1".into(),
                                source: crate::protocol::api::schema::ReadSource::RecentUnwrapped,
                                format: crate::protocol::api::schema::ReadFormat::Text,
                                text: String::new(),
                                revision: 0,
                                truncated: false,
                                viewport_rows: None,
                                viewport_columns: None,
                                requested_lines: None,
                                returned_lines: 0,
                                available_lines: None,
                                exhausted: None,
                            },
                        },
                    })
                    .unwrap(),
                )
                .unwrap();
        }
    });

    let (mut client, server, _path) = local_stream_pair("api-wait-disconnect");
    client
        .write_all(br#"{"id":"req_wait","method":"pane.wait_for_output","params":{"pane_id":"pane_1","source":"recent","match":{"type":"substring","value":"never"}}}"#)
        .unwrap();
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let event_hub = EventHub::default();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let server_thread = std::thread::spawn(move || {
        let result = handle_connection(server, &api_tx, &event_hub, &server_running);
        done_tx.send(result).unwrap();
    });

    first_read_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(client);

    let result = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(result.is_ok());

    server_thread.join().unwrap();
    drop(running);
    responder.join().unwrap();
}

#[test]
fn subscriptions_stop_when_client_disconnects() {
    let (api_tx, _api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
    let (mut client, server, _path) = local_stream_pair("api-sub-disconnect");
    client
        .write_all(
            br#"{"id":"sub_1","method":"events.subscribe","params":{"subscriptions":[{"type":"workspace.created"}]}}"#,
        )
        .unwrap();
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let event_hub = EventHub::default();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let server_thread = std::thread::spawn(move || {
        let result = handle_connection(server, &api_tx, &event_hub, &server_running);
        done_tx.send(result).unwrap();
    });

    let ack = read_line(&mut client);
    let ack: serde_json::Value = serde_json::from_str(&ack).unwrap();
    assert_eq!(ack["result"]["type"], "subscription_started");

    drop(client);

    let result = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(result.is_ok());
    server_thread.join().unwrap();
}

#[test]
fn subscriptions_stop_when_server_shuts_down() {
    let (api_tx, _api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
    let (mut client, server, _path) = local_stream_pair("api-sub-shutdown");
    client
        .write_all(
            br#"{"id":"sub_2","method":"events.subscribe","params":{"subscriptions":[{"type":"workspace.created"}]}}"#,
        )
        .unwrap();
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let event_hub = EventHub::default();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let server_thread = std::thread::spawn(move || {
        let result = handle_connection(server, &api_tx, &event_hub, &server_running);
        done_tx.send(result).unwrap();
    });

    let ack = read_line(&mut client);
    let ack: serde_json::Value = serde_json::from_str(&ack).unwrap();
    assert_eq!(ack["result"]["type"], "subscription_started");

    running.store(false, Ordering::Relaxed);

    let result = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(result.is_ok());
    server_thread.join().unwrap();
}
