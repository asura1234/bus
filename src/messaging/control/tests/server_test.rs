use super::*;
use crate::platform::ipc;
use interprocess::local_socket::traits::Stream as _;
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        // Hex keeps the socket path short; pid and counter separate parallel processes.
        let dir = std::env::temp_dir().join(format!(
            "bdc-{:x}-{:x}-{:x}",
            std::process::id(),
            storage_io::now_ns(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        Self(dir)
    }
    fn socket(&self) -> PathBuf {
        self.0.join("control.sock")
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn call(id: &str) -> Request {
    Request {
        id: id.into(),
        method: "state.get".into(),
        params: Value::Null,
    }
}

fn read_response(mut stream: ipc::LocalStream) -> Response {
    read_response_open(&mut stream)
}

fn read_response_open(stream: &mut ipc::LocalStream) -> Response {
    stream.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    let mut buf = [0; 8192];
    loop {
        match ipc::poll_local_stream_read_count(stream, &mut buf).unwrap() {
            ipc::LocalStreamReadCount::Data(n) => {
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                    return serde_json::from_slice(&bytes[..end]).unwrap();
                }
            }
            ipc::LocalStreamReadCount::Pending => thread::sleep(Duration::from_millis(2)),
            ipc::LocalStreamReadCount::Closed => panic!("closed without response"),
        }
        assert!(Instant::now() < deadline, "response deadline elapsed");
    }
}

#[cfg(unix)]
#[test]
fn every_session_listens_on_control_sock() {
    let dir = TestDir::new();
    let (tx, _rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    assert!(dir.socket().exists());
    assert!(dir.0.join(WAIT_SOCKET_NAME).exists());
    assert!(!dir.0.join("dev-control.sock").exists());
}

#[cfg(unix)]
#[test]
fn client_reaches_a_bus_still_on_the_legacy_socket_name() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    // A Bus started before the rename listens on dev-control.sock.
    std::fs::rename(dir.socket(), dir.0.join("dev-control.sock")).unwrap();
    let worker = thread::spawn(move || {
        let (_, BusCommand::Control(call)) = rx.recv().unwrap() else {
            panic!("expected a control call")
        };
        call.reply
            .send(Response::success(&call.request.id, json!({"legacy": true})))
            .unwrap();
    });
    let response = request(&dir.0, &call("legacy")).unwrap();
    assert_eq!(response.result, json!({"legacy": true}));
    worker.join().unwrap();
}

#[test]
fn client_cannot_enable_target_or_remove_existing_endpoint() {
    let dir = TestDir::new();
    assert!(request(&dir.0, &call("missing")).is_err());
    assert!(!dir.0.exists());
    std::fs::create_dir(&dir.0).unwrap();
    std::fs::write(dir.socket(), b"existing endpoint marker").unwrap();
    assert!(request(&dir.0, &call("stopped")).is_err());
    assert_eq!(
        std::fs::read(dir.socket()).unwrap(),
        b"existing endpoint marker"
    );
}

#[test]
fn roundtrip_returns_exact_worker_outcome_with_disjoint_command_ids() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(2);
    let server = start(&dir.0, tx).unwrap();
    let worker = thread::spawn(move || {
        for expected_id in ["snapshot", "bad-command"] {
            let (command_id, BusCommand::Control(dev)) =
                rx.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("wrong coordinator command");
            };
            assert!(command_id & (1_u64 << 63) != 0);
            assert_eq!(dev.request.id, expected_id);
            assert_eq!(dev.request.method, "state.get");
            let response = if expected_id == "snapshot" {
                Response::success(expected_id, json!({"revision": 7, "room": "alpha"}))
            } else {
                Response::failure(expected_id, "invalid_params", "Room is missing")
            };
            dev.reply.try_send(response).unwrap();
        }
    });
    let ok = request(&dir.0, &call("snapshot")).unwrap();
    assert_eq!(ok.id, "snapshot");
    assert!(ok.ok);
    assert_eq!(ok.result, json!({"revision": 7, "room": "alpha"}));
    assert!(ok.error.is_none());
    let failed = request(&dir.0, &call("bad-command")).unwrap();
    assert!(!failed.ok);
    assert_eq!(failed.id, "bad-command");
    assert_eq!(failed.error.unwrap().code, "invalid_params");
    worker.join().unwrap();
    drop(server);
    assert!(!dir.socket().exists());
    assert!(!dir.0.join(WAIT_SOCKET_NAME).exists());
}

#[test]
fn malformed_and_idle_clients_do_not_delay_healthy_requests() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(2);
    let _server = start(&dir.0, tx).unwrap();
    let _idle = ipc::connect_local_stream(&dir.socket()).unwrap();
    let mut malformed = ipc::connect_local_stream(&dir.socket()).unwrap();
    malformed.write_all(b"{secret invalid json}\n").unwrap();
    let failed = read_response(malformed);
    assert!(!failed.ok);
    let error = failed.error.unwrap();
    assert_eq!(error.code, "invalid_request");
    assert!(!error.message.contains("secret"));
    let worker = thread::spawn(move || {
        let (_, BusCommand::Control(dev)) = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!()
        };
        dev.reply
            .try_send(Response::success(&dev.request.id, json!("healthy")))
            .unwrap();
    });
    assert_eq!(
        request(&dir.0, &call("healthy")).unwrap().result,
        json!("healthy")
    );
    worker.join().unwrap();
}

#[test]
fn oversized_request_is_rejected_without_coordinator_dispatch() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    let mut stream = ipc::connect_local_stream(&dir.socket()).unwrap();
    stream.write_all(&vec![b'x'; 256 * 1024 + 1]).unwrap();
    let response = read_response(stream);
    assert_eq!(response.error.unwrap().code, "request_too_large");
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let mut large = call("large");
    large.params = json!("x".repeat(256 * 1024));
    assert!(request(&dir.0, &large)
        .unwrap_err()
        .contains("request_too_large"));
}

#[test]
fn full_or_disconnected_coordinator_returns_explicit_error() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(0);
    let _server = start(&dir.0, tx).unwrap();
    assert_eq!(
        request(&dir.0, &call("full")).unwrap().error.unwrap().code,
        "coordinator_busy"
    );
    drop(rx);
    assert_eq!(
        request(&dir.0, &call("gone")).unwrap().error.unwrap().code,
        "coordinator_unavailable"
    );
}

#[test]
fn oversized_worker_response_is_replaced_with_bounded_error() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    let worker = thread::spawn(move || {
        let (_, BusCommand::Control(dev)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!()
        };
        dev.reply
            .try_send(Response::success(
                &dev.request.id,
                json!("x".repeat(16 * 1024 * 1024)),
            ))
            .unwrap();
    });
    let failed = request(&dir.0, &call("large")).unwrap();
    assert_eq!(failed.id, "large");
    assert_eq!(failed.error.unwrap().code, "response_too_large");
    worker.join().unwrap();
}

#[test]
fn second_server_cannot_replace_a_live_listener() {
    let dir = TestDir::new();
    let (tx, _rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx.clone()).unwrap();
    let before = ipc::socket_file_identity(&dir.socket()).unwrap();
    assert!(start(&dir.0, tx).is_err());
    assert_eq!(ipc::socket_file_identity(&dir.socket()).unwrap(), before);
}

#[test]
fn dropping_server_cancels_idle_clients_and_preserves_replacement_endpoint() {
    let dir = TestDir::new();
    let (tx, _rx) = mpsc::sync_channel(1);
    let server = start(&dir.0, tx).unwrap();
    let _idle = ipc::connect_local_stream(&dir.socket()).unwrap();
    std::fs::rename(dir.socket(), dir.0.join("old.sock")).unwrap();
    std::fs::write(dir.socket(), b"replacement endpoint").unwrap();
    let began = Instant::now();
    drop(server);
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(
        std::fs::read(dir.socket()).unwrap(),
        b"replacement endpoint"
    );
}

#[test]
fn idle_request_expires_without_dispatch() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    let idle = ipc::connect_local_stream(&dir.socket()).unwrap();
    assert_eq!(read_response(idle).error.unwrap().code, "request_timeout");
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn unanswered_coordinator_request_has_finite_deadline() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    let result = request(&dir.0, &call("never-finished")).unwrap();
    assert_eq!(result.error.unwrap().code, "coordinator_timeout");
    let (_, BusCommand::Control(dev)) = rx.try_recv().unwrap() else {
        panic!()
    };
    assert!(dev
        .reply
        .try_send(Response::success("never-finished", Value::Null))
        .is_err());
}

#[test]
fn active_connection_limit_rejects_excess_clients() {
    for (socket, method) in [
        (SOCKET_NAME, "state.get"),
        (WAIT_SOCKET_NAME, "message.status"),
    ] {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(MAX_CLIENTS);
        let _server = start(&dir.0, tx).unwrap();
        let mut clients = Vec::new();
        let mut pending = Vec::new();
        for id in 0..MAX_CLIENTS {
            let mut client = ipc::connect_local_stream(&dir.0.join(socket)).unwrap();
            let mut request = call(&format!("pending-{id}"));
            request.method = method.into();
            writeln!(client, "{}", serde_json::to_string(&request).unwrap()).unwrap();
            pending.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
            clients.push(client);
        }
        let excess = ipc::connect_local_stream(&dir.0.join(socket)).unwrap();
        assert_eq!(read_response(excess).error.unwrap().code, "server_busy");
        assert_eq!(pending.len(), MAX_CLIENTS);
    }
}

#[test]
fn overload_logs_the_endpoint_and_requests_holding_its_slots() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let (listener, _socket) = bind_endpoint(&dir.0, SOCKET_NAME).unwrap();
    let mut endpoint = Endpoint {
        listener: Some(listener),
        clients: vec![],
        poll_budget: None,
        last_overload_log: None,
    };
    let (tx, _rx) = mpsc::sync_channel(MAX_CLIENTS);
    let mut next = CONTROL_COMMAND_BIT;
    let mut peers = vec![];
    for n in 0..MAX_CLIENTS {
        let mut peer = ipc::connect_local_stream(&dir.socket()).unwrap();
        let request = Request {
            id: format!("bus-123-{n}"),
            method: "state".into(),
            params: json!({"secret":"private draft"}),
        };
        writeln!(peer, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        peers.push(peer);
        endpoint.poll(&tx, &mut next);
    }
    let excess = ipc::connect_local_stream(&dir.socket()).unwrap();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("info", || endpoint.poll(&tx, &mut next));
    let text = capture.text();
    assert!(text.contains("bus.control.admission.rejected"), "{text}");
    assert!(
        text.contains("endpoint=\"interactive\"")
            && text.contains("active=16")
            && text.contains("waiting=16"),
        "{text}"
    );
    assert!(
        text.contains("bus-123-0") && text.contains("bus-123-15") && text.contains("state"),
        "{text}"
    );
    assert!(
        !text.contains("private draft") && !text.contains("secret"),
        "{text}"
    );
    drop(excess);
}

#[test]
fn coordinator_deadline_logs_the_request_without_its_params() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let (listener, _socket) = bind_endpoint(&dir.0, SOCKET_NAME).unwrap();
    let _peer = ipc::connect_local_stream(&dir.socket()).unwrap();
    let mut client = Client::new(listener.accept().unwrap());
    let request = Request {
        id: "bus-321-timeout".into(),
        method: "state".into(),
        params: json!({"secret":"private draft"}),
    };
    let (tx, _rx) = mpsc::sync_channel(1);
    let mut next = CONTROL_COMMAND_BIT;
    assert!(client.dispatch_request(&serde_json::to_vec(&request).unwrap(), &tx, &mut next, None));
    client.deadline = Instant::now() - Duration::from_millis(1);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("info", || {
        assert!(client.poll(&tx, &mut next, None));
    });
    let text = capture.text();
    assert!(
        text.contains("bus.control.request.timeout")
            && text.contains("bus-321-timeout")
            && text.contains("method=\"state\""),
        "{text}"
    );
    assert!(
        !text.contains("private draft") && !text.contains("secret"),
        "{text}"
    );
}

#[test]
fn coordinator_deadline_log_caps_the_request_id_like_overload_records() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let (listener, _socket) = bind_endpoint(&dir.0, SOCKET_NAME).unwrap();
    let _peer = ipc::connect_local_stream(&dir.socket()).unwrap();
    let mut client = Client::new(listener.accept().unwrap());
    let request = Request {
        id: "x".repeat(200 * 1024),
        method: "state".into(),
        params: json!({}),
    };
    let (tx, _rx) = mpsc::sync_channel(1);
    let mut next = CONTROL_COMMAND_BIT;
    assert!(client.dispatch_request(&serde_json::to_vec(&request).unwrap(), &tx, &mut next, None));
    client.deadline = Instant::now() - Duration::from_millis(1);
    // Keep callsite interest global while other libtest threads exercise the
    // same log site without a thread-local subscriber.
    let _callsite_registration = tracing::Dispatch::new(tracing_subscriber::registry());
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("info", || {
        assert!(client.poll(&tx, &mut next, None));
    });
    let text = capture.text();
    assert!(
        text.contains("bus.control.request.timeout"),
        "{}",
        &text[..text.len().min(500)]
    );
    assert!(
        text.len() < 4096,
        "timeout record carries an uncapped {} byte request id",
        text.len()
    );
    assert!(
        text.contains(&"x".repeat(128)),
        "the useful ID prefix must remain"
    );
    assert!(
        !text.contains(&"x".repeat(129)),
        "the log ID must stop at 128 characters"
    );
    let State::Writing { bytes, .. } = &client.state else {
        panic!("timeout response must be ready to write")
    };
    let response: Response = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        response.id, request.id,
        "wire correlation keeps the full ID"
    );
}

#[test]
fn saturated_status_endpoint_leaves_headroom_for_an_interrupt() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(64);
    let _server = start(&dir.0, tx).unwrap();
    let mut clients = Vec::new();
    let mut pending = Vec::new();
    for id in 0..16 {
        let mut stream = ipc::connect_local_stream(&dir.0.join(WAIT_SOCKET_NAME)).unwrap();
        let request = Request {
            id: format!("follower-{id}"),
            method: "message.status".into(),
            params: json!({"message": "19"}),
        };
        writeln!(stream, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(call) => pending.push(call),
            Err(error) => panic!(
                "status poll {id} must be admitted before an interrupt: {error}; response: {:?}",
                read_response(stream)
            ),
        }
        clients.push(stream);
    }
    // The coordinator is slow to answer the followers; their turns may last
    // minutes, but only this status read occupies each connection.
    let mut interrupt = ipc::connect_local_stream(&dir.socket()).unwrap();
    let request = Request {
        id: "interactive-interrupt".into(),
        method: "agent.interrupt".into(),
        params: json!({"agent": "worker"}),
    };
    writeln!(interrupt, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("expected an interrupt control call")
    };
    assert_eq!(call.request.method, "agent.interrupt");
    call.reply
        .send(Response::success(
            &call.request.id,
            json!({"admitted": true}),
        ))
        .unwrap();
    let response = read_response(interrupt);
    assert!(response.ok, "{response:?}");
    assert_eq!(response.id, "interactive-interrupt");
    assert_eq!(response.result, json!({"admitted": true}));
    assert_eq!(pending.len(), 16);
    assert_eq!(clients.len(), 16);
}

#[test]
fn caller_deadline_bounds_an_unanswered_request() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    let path = dir.0.clone();
    let started = Instant::now();
    let client = thread::spawn(move || {
        request_with_timeout(&path, &call("deadline"), Duration::from_millis(100))
    });
    let pending = rx.recv_timeout(Duration::from_secs(1));
    let error = client.join().unwrap().unwrap_err();
    assert!(error.contains("response_timeout"), "{error}");
    assert!(
        pending.is_ok(),
        "request must reach the real coordinator boundary"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn client_rejects_a_response_for_another_request() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = start(&dir.0, tx).unwrap();
    let worker = thread::spawn(move || {
        let (_, BusCommand::Control(dev)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!()
        };
        dev.reply
            .try_send(Response::success(
                "some-other-request",
                json!({"accepted":true}),
            ))
            .unwrap();
    });
    assert!(request(&dir.0, &call("expected"))
        .unwrap_err()
        .contains("invalid_response"));
    worker.join().unwrap();
}

#[test]
fn completed_responses_remain_counted_until_peers_close() {
    for (socket, method) in [
        (SOCKET_NAME, "state.get"),
        (WAIT_SOCKET_NAME, "message.status"),
    ] {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(MAX_CLIENTS);
        let _server = start(&dir.0, tx).unwrap();
        let mut clients = Vec::new();
        for id in 0..MAX_CLIENTS {
            let mut stream = ipc::connect_local_stream(&dir.0.join(socket)).unwrap();
            let mut request = call(&format!("retain-{id}"));
            request.method = method.into();
            writeln!(stream, "{}", serde_json::to_string(&request).unwrap()).unwrap();
            let (_, BusCommand::Control(dev)) = rx.recv_timeout(Duration::from_secs(2)).unwrap()
            else {
                panic!()
            };
            dev.reply
                .try_send(Response::success(&dev.request.id, json!("ready")))
                .unwrap();
            assert!(read_response_open(&mut stream).ok);
            clients.push(stream);
        }
        let excess = ipc::connect_local_stream(&dir.0.join(socket)).unwrap();
        assert_eq!(read_response(excess).error.unwrap().code, "server_busy");
    }
}

/// With every slot and the overload reply slot taken, a further caller must
/// not be closed unanswered: it could not tell whether its mutation ran.
#[test]
fn excess_caller_waits_for_a_slot_instead_of_being_closed_unanswered() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(32);
    let _server = start(&dir.0, tx).unwrap();
    let mut streams = Vec::new();
    let mut pending = Vec::new();
    for id in 0..16 {
        let mut stream = ipc::connect_local_stream(&dir.socket()).unwrap();
        writeln!(
            stream,
            "{}",
            serde_json::to_string(&call(&format!("stalled-{id}"))).unwrap()
        )
        .unwrap();
        pending.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        streams.push(stream);
    }
    // Another overloaded caller holds the single server_busy reply slot.
    let busy_holder = ipc::connect_local_stream(&dir.socket()).unwrap();
    thread::sleep(Duration::from_millis(100));
    let path = dir.0.clone();
    let add = thread::spawn(move || {
        request(
            &path,
            &Request {
                id: "add".into(),
                method: "agent.add".into(),
                params: Value::Null,
            },
        )
    });
    thread::sleep(Duration::from_millis(300));
    assert!(!add.is_finished(), "excess caller must wait for a slot");
    drop(busy_holder);
    // The freed slot answers the waiting caller with an explicit overload
    // reply, which proves its request never reached the coordinator.
    let outcome = add.join().unwrap().unwrap();
    assert_eq!(outcome.error.unwrap().code, "server_busy");
    assert_eq!(pending.len(), 16);
    drop(streams);
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn startup_reclaims_a_closed_listener_after_obtaining_its_lease() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let old = ipc::bind_private_local_listener(&dir.socket()).unwrap();
    drop(old);
    assert!(
        dir.socket().exists(),
        "fixture must retain the crashed listener endpoint"
    );
    let (tx, _rx) = mpsc::sync_channel(0);
    let _server = start(&dir.0, tx).unwrap();
    assert_eq!(
        request(&dir.0, &call("recovered"))
            .unwrap()
            .error
            .unwrap()
            .code,
        "coordinator_busy"
    );
}

#[test]
fn startup_preserves_a_live_listener_without_our_lease() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let _listener = ipc::bind_private_local_listener(&dir.socket()).unwrap();
    let identity = ipc::socket_file_identity(&dir.socket()).unwrap();
    let (tx, _rx) = mpsc::sync_channel(0);
    assert!(start(&dir.0, tx).is_err());
    assert_eq!(ipc::socket_file_identity(&dir.socket()).unwrap(), identity);
}

#[test]
fn startup_preserves_a_non_socket_endpoint_file() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    std::fs::write(dir.socket(), b"user data").unwrap();
    let (tx, _rx) = mpsc::sync_channel(0);
    assert!(start(&dir.0, tx).is_err());
    assert_eq!(std::fs::read(dir.socket()).unwrap(), b"user data");
}

#[test]
fn control_requests_are_timed_from_dispatch_to_reply() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(2);
    let server = start(&dir.0, tx).unwrap();
    let worker = thread::spawn(move || {
        let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("wrong coordinator command");
        };
        thread::sleep(Duration::from_millis(120));
        call.reply
            .try_send(Response::success("slow", json!({})))
            .unwrap();
    });
    assert!(request(&dir.0, &call("slow")).unwrap().ok);
    worker.join().unwrap();
    drop(server);
    let window = crate::messaging::perf::CONTROL_REQUEST
        .take()
        .expect("request timed");
    assert!(window.count >= 1);
    assert!(window.max_us >= 120_000, "{window:?}");
}

#[test]
fn slow_unknown_control_methods_do_not_log_user_supplied_text() {
    use interprocess::local_socket::traits::Listener as _;
    let secret = "private draft text accidentally passed as method";
    let slow_unknown_request = |capture: &crate::utils::logging::test_capture::Capture| {
        let dir = TestDir::new();
        storage_io::private_dir(&dir.0).unwrap();
        let listener = ipc::bind_private_local_listener(&dir.socket()).unwrap();
        let _peer = ipc::connect_local_stream(&dir.socket()).unwrap();
        let mut client = Client::new(listener.accept().unwrap());
        let (tx, rx) = mpsc::sync_channel(1);
        let request = Request {
            id: "unknown".into(),
            method: secret.into(),
            params: Value::Null,
        };
        let mut next_id = CONTROL_COMMAND_BIT;
        assert!(client.dispatch_request(
            &serde_json::to_vec(&request).unwrap(),
            &tx,
            &mut next_id,
            None
        ));
        let (_, BusCommand::Control(call)) = rx.recv().unwrap() else {
            panic!("control call")
        };
        thread::sleep(Duration::from_millis(120));
        call.reply
            .try_send(Response::failure(
                "unknown",
                "unknown_method",
                "Unknown method",
            ))
            .unwrap();
        capture.run(|| {
            assert!(client.poll(&tx, &mut next_id, None));
        });
    };
    // CONTROL_REQUEST WARNs are throttled to one per second per process, and
    // concurrent tests in this binary record slow control requests too; once
    // the window that swallowed this request's WARN has passed, retry.
    let capture = crate::utils::logging::test_capture::Capture::default();
    for _ in 0..5 {
        slow_unknown_request(&capture);
        if capture.text().contains("perf.slow_op") {
            break;
        }
        thread::sleep(Duration::from_millis(1_100));
    }
    let text = capture.text();
    assert!(
        text.contains("perf.slow_op"),
        "slow request must remain diagnosable: {text}"
    );
    assert!(
        !text.contains(secret),
        "user supplied method leaked into WARN: {text}"
    );
}

#[test]
fn timed_out_status_connections_cannot_accumulate_coordinator_commands() {
    use interprocess::local_socket::traits::Listener as _;
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let listener = ipc::bind_private_local_listener(&dir.socket()).unwrap();
    let (tx, rx) = mpsc::sync_channel(256);
    let budget = Arc::new(());
    let mut next_id = CONTROL_COMMAND_BIT;
    let mut pending = Vec::new();
    // Model transports reaching their coordinator deadline while their commands
    // remain queued. Retries must not consume the interactive queue's capacity.
    for id in 0..100 {
        let peer = ipc::connect_local_stream(&dir.socket()).unwrap();
        let mut client = Client::new(listener.accept().unwrap());
        let request = Request {
            id: id.to_string(),
            method: "message.status".into(),
            params: json!({"message": "1"}),
        };
        assert!(client.dispatch_request(
            &serde_json::to_vec(&request).unwrap(),
            &tx,
            &mut next_id,
            Some(&budget)
        ));
        if id < 16 {
            pending.push(rx.recv_timeout(Duration::from_secs(1)).unwrap());
            client.deadline = Instant::now();
            assert!(client.poll(&tx, &mut next_id, Some(&budget)));
        }
        assert!(client.poll(&tx, &mut next_id, Some(&budget)));
        let response = read_response(peer);
        assert_eq!(
            response.error.unwrap().code,
            if id < 16 {
                "coordinator_timeout"
            } else {
                "server_busy"
            }
        );
        assert!(
            rx.try_recv().is_err(),
            "timed-out retries must not enqueue more polls"
        );
    }
    // Completing one old command releases exactly enough budget for a retry.
    pending.pop();
    let _peer = ipc::connect_local_stream(&dir.socket()).unwrap();
    let mut client = Client::new(listener.accept().unwrap());
    let request = Request {
        id: "retry".into(),
        method: "message.status".into(),
        params: json!({"message": "1"}),
    };
    assert!(client.dispatch_request(
        &serde_json::to_vec(&request).unwrap(),
        &tx,
        &mut next_id,
        Some(&budget)
    ));
    let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("expected retry")
    };
    assert_eq!(call.request.id, "retry");
}

#[test]
fn control_endpoints_enforce_their_method_boundary() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(256);
    let _server = start(&dir.0, tx).unwrap();
    for (socket, method) in [
        (SOCKET_NAME, "message.status"),
        (WAIT_SOCKET_NAME, "message.send"),
    ] {
        let mut stream = ipc::connect_local_stream(&dir.0.join(socket)).unwrap();
        let request = Request {
            id: "wrong-endpoint".into(),
            method: method.into(),
            params: json!({}),
        };
        writeln!(stream, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(read_response(stream).error.unwrap().code, "invalid_request");
    }
    assert!(
        rx.try_recv().is_err(),
        "wrong-endpoint calls must never be dispatched"
    );
}

#[test]
fn failed_poll_endpoint_startup_removes_only_the_new_interactive_socket() {
    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let path = dir.0.join(WAIT_SOCKET_NAME);
    std::fs::write(&path, b"user data").unwrap();
    let (tx, _) = mpsc::sync_channel(0);
    assert!(start(&dir.0, tx).is_err());
    assert!(!dir.socket().exists());
    assert_eq!(std::fs::read(&path).unwrap(), b"user data");
    std::fs::remove_file(&path).unwrap();
    let (tx, _) = mpsc::sync_channel(0);
    let server = start(&dir.0, tx).unwrap();
    drop(server);
    assert!(!dir.socket().exists());
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn control_server_starts_when_the_interactive_socket_path_fits() {
    use std::os::unix::ffi::OsStrExt;
    let dir = TestDir::new();
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let max_socket_bytes = address.sun_path.len() - 1;
    let parent_bytes = max_socket_bytes - SOCKET_NAME.len() - 1;
    let padding = parent_bytes
        .checked_sub(dir.0.as_os_str().as_bytes().len() + 1)
        .expect("fixture must leave room for a near-limit private data directory");
    let data_dir = dir.0.join("p".repeat(padding));
    storage_io::private_dir(&data_dir).unwrap();
    let socket = data_dir.join(SOCKET_NAME);
    assert_eq!(socket.as_os_str().as_bytes().len(), max_socket_bytes);
    let listener = ipc::bind_private_local_listener(&socket)
        .expect("the existing interactive socket name must fit");
    drop(listener);
    std::fs::remove_file(socket).unwrap();
    let (tx, _rx) = mpsc::sync_channel(1);
    let result = start(&data_dir, tx);
    assert!(
        result.is_ok(),
        "a data directory supporting control.sock must still start: {:?}",
        result.err()
    );
}

#[cfg(unix)]
#[test]
fn failed_waiter_accept_keeps_the_interactive_endpoint_serving() {
    use interprocess::os::unix::uds_local_socket;
    use std::os::{fd::OwnedFd, unix::net::UnixStream};

    let dir = TestDir::new();
    storage_io::private_dir(&dir.0).unwrap();
    let lease = storage_io::lock(&dir.0.join(LOCK_NAME)).unwrap();
    let (interactive, socket) = bind_endpoint(&dir.0, SOCKET_NAME).unwrap();
    let (waiter, wait_socket) = bind_endpoint(&dir.0, WAIT_SOCKET_NAME).unwrap();
    drop(waiter);
    // A connected Unix socket is valid but cannot accept: exercise a real
    // non-transient kernel error without exhausting process-wide descriptors.
    let (stream, _peer) = UnixStream::pair().unwrap();
    let failed: ipc::LocalListener = uds_local_socket::Listener::from(OwnedFd::from(stream)).into();
    let error = failed.accept().unwrap_err();
    assert!(!matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    ));

    let (tx, rx) = mpsc::sync_channel(2);
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let thread = thread::spawn(move || serve([interactive, failed], tx, stop));
    let _server = Server {
        stopped,
        thread: Some(thread),
        _sockets: [socket, wait_socket],
        _lease: lease,
    };
    thread::sleep(Duration::from_millis(50));
    let worker = thread::spawn(move || {
        for id in ["interrupt", "new-send"] {
            let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("expected interactive control call")
            };
            assert_eq!(call.request.id, id);
            call.reply
                .send(Response::success(id, json!({"served": id})))
                .unwrap();
        }
    });
    for (id, method) in [
        ("interrupt", "agent.interrupt"),
        ("new-send", "message.send"),
    ] {
        let response = request(
            &dir.0,
            &Request {
                id: id.into(),
                method: method.into(),
                params: json!({}),
            },
        )
        .expect("a failed waiter listener must not stop interactive control");
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result, json!({"served": id}));
    }
    worker.join().unwrap();
}

fn status_request(id: &str) -> Request {
    Request {
        id: id.into(),
        method: "message.status".into(),
        params: json!({"message": "1"}),
    }
}

#[test]
fn coordinator_stopping_answers_queued_status_polls() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(256);
    let _server = start(&dir.0, tx).unwrap();
    let wait = dir.0.join(WAIT_SOCKET_NAME);
    let mut streams = Vec::new();
    for id in 0..4 {
        let mut stream = ipc::connect_local_stream(&wait).unwrap();
        writeln!(
            stream,
            "{}",
            serde_json::to_string(&status_request(&format!("s-{id}"))).unwrap()
        )
        .unwrap();
        streams.push(stream);
    }
    let mut calls = Vec::new();
    for _ in 0..4 {
        calls.push(rx.recv_timeout(Duration::from_secs(2)).unwrap());
    }
    // Bus shutdown: the coordinator drops its receiver and pending calls.
    drop(calls);
    drop(rx);
    for stream in streams {
        let response = read_response(stream);
        assert_eq!(response.error.unwrap().code, "coordinator_unavailable");
    }
    let mut late = ipc::connect_local_stream(&wait).unwrap();
    writeln!(
        late,
        "{}",
        serde_json::to_string(&status_request("late")).unwrap()
    )
    .unwrap();
    assert_eq!(
        read_response(late).error.unwrap().code,
        "coordinator_unavailable"
    );
}

#[test]
fn saturated_interactive_pool_does_not_block_status_polls() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(256);
    let _server = start(&dir.0, tx).unwrap();
    let mut idle = Vec::new();
    for _ in 0..=MAX_CLIENTS + 4 {
        idle.push(ipc::connect_local_stream(&dir.socket()).unwrap());
    }
    thread::sleep(Duration::from_millis(50));
    let mut stream = ipc::connect_local_stream(&dir.0.join(WAIT_SOCKET_NAME)).unwrap();
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&status_request("poll")).unwrap()
    )
    .unwrap();
    let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!()
    };
    assert_eq!(call.request.method, "message.status");
    assert!(call._poll_permit.is_some());
    call.reply
        .send(Response::success("poll", json!({})))
        .unwrap();
    assert!(read_response(stream).ok);
}

/// Status reads are repeatable, so a saturated status endpoint closes an
/// excess poll at once for its follower to retry, instead of leaving it in a
/// listen backlog that can fill and refuse a live Bus's pollers.
#[test]
fn saturated_status_endpoint_closes_excess_polls_promptly() {
    let dir = TestDir::new();
    let (tx, _rx) = mpsc::sync_channel(256);
    let _server = start(&dir.0, tx).unwrap();
    let mut idle = Vec::new();
    for _ in 0..=MAX_CLIENTS {
        idle.push(ipc::connect_local_stream(&dir.0.join(WAIT_SOCKET_NAME)).unwrap());
    }
    thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    let outcome = request(&dir.0, &status_request("excess"));
    assert!(
        outcome
            .as_ref()
            .is_err_and(|e| e.starts_with("control_io:")),
        "{outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// A caller that already gave up while waiting for a slot reported a
/// failure; its buffered request must not run afterwards.
#[test]
fn excess_caller_that_timed_out_waiting_never_runs_later() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(32);
    let _server = start(&dir.0, tx).unwrap();
    let mut streams = Vec::new();
    let mut pending = Vec::new();
    for id in 0..16 {
        let mut stream = ipc::connect_local_stream(&dir.socket()).unwrap();
        writeln!(
            stream,
            "{}",
            serde_json::to_string(&call(&format!("stalled-{id}"))).unwrap()
        )
        .unwrap();
        let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("wrong coordinator command");
        };
        pending.push(call);
        streams.push(stream);
    }
    let busy_holder = ipc::connect_local_stream(&dir.socket()).unwrap();
    thread::sleep(Duration::from_millis(100));
    let outcome = request_with_timeout(
        &dir.0,
        &Request {
            id: "late-add".into(),
            method: "agent.add".into(),
            params: Value::Null,
        },
        Duration::from_millis(300),
    );
    assert!(
        outcome
            .as_ref()
            .is_err_and(|e| e.starts_with("response_timeout:")),
        "{outcome:?}"
    );
    // Regular slots free after the caller has already reported failure.
    for _ in 0..2 {
        let call = pending.remove(0);
        call.reply
            .try_send(Response::success(&call.request.id, json!({})))
            .unwrap();
        drop(streams.remove(0));
    }
    let late = rx.recv_timeout(Duration::from_secs(2));
    assert!(
        !matches!(&late, Ok((_, BusCommand::Control(call))) if call.request.id == "late-add"),
        "the abandoned agent.add was dispatched after its caller timed out"
    );
    drop(busy_holder);
}

#[test]
fn shell_launch_control_deadline_encloses_startup_and_preserves_queue_age() {
    for (method, budget) in [
        (
            "agent.add",
            crate::protocol::api::AGENT_START_TIMEOUT + Duration::from_secs(15),
        ),
        (
            "agent.resume",
            crate::protocol::api::AGENT_START_TIMEOUT + Duration::from_secs(15),
        ),
        ("state.get", Duration::from_secs(15)),
    ] {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(&dir.0, tx).unwrap();
        let path = dir.0.clone();
        let before = Instant::now();
        let client = thread::spawn(move || {
            request(
                &path,
                &Request {
                    id: "launch-budget".into(),
                    method: method.into(),
                    params: json!({}),
                },
            )
        });
        let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(60)).unwrap()
        else {
            panic!("expected control dispatch");
        };
        let after = Instant::now();
        let deadline = call.deadline.unwrap();
        assert!(
            deadline >= before + budget && deadline <= after + budget,
            "{method} dispatch deadline must enclose its operation budget"
        );
        let minimum_age = after.elapsed();
        let age = call.queued_for().unwrap();
        let maximum_age = before.elapsed();
        assert!(
            age >= minimum_age && age <= maximum_age,
            "{method} queue age {age:?} must be anchored between request and dispatch observations"
        );
        call.reply
            .send(Response::failure(
                "launch-budget",
                "agent_shell_waiting",
                "Shell waiting. Last visible line: STARTUP QUESTION",
            ))
            .unwrap();
        let response = client.join().unwrap().unwrap();
        assert_eq!(
            response.error.unwrap().message,
            "Shell waiting. Last visible line: STARTUP QUESTION"
        );
    }
}
