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
        let (_, BusCommand::Dev(call)) = rx.recv().unwrap() else {
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
            let (command_id, BusCommand::Dev(dev)) =
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
        let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
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
        let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
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
    let (_, BusCommand::Dev(dev)) = rx.try_recv().unwrap() else {
        panic!()
    };
    assert!(dev
        .reply
        .try_send(Response::success("never-finished", Value::Null))
        .is_err());
}

#[test]
fn active_connection_limit_rejects_excess_clients() {
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(16);
    let _server = start(&dir.0, tx).unwrap();
    let mut clients = Vec::new();
    let mut pending = Vec::new();
    for id in 0..16 {
        let mut client = ipc::connect_local_stream(&dir.socket()).unwrap();
        writeln!(
            client,
            "{}",
            serde_json::to_string(&call(&format!("pending-{id}"))).unwrap()
        )
        .unwrap();
        pending.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        clients.push(client);
    }
    let excess = ipc::connect_local_stream(&dir.socket()).unwrap();
    assert_eq!(read_response(excess).error.unwrap().code, "server_busy");
    assert_eq!(pending.len(), 16);
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
        let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
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
    let dir = TestDir::new();
    let (tx, rx) = mpsc::sync_channel(16);
    let _server = start(&dir.0, tx).unwrap();
    let mut clients = Vec::new();
    for id in 0..16 {
        let mut stream = ipc::connect_local_stream(&dir.socket()).unwrap();
        writeln!(
            stream,
            "{}",
            serde_json::to_string(&call(&format!("retain-{id}"))).unwrap()
        )
        .unwrap();
        let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!()
        };
        dev.reply
            .try_send(Response::success(&dev.request.id, json!("ready")))
            .unwrap();
        assert!(read_response_open(&mut stream).ok);
        clients.push(stream);
    }
    let excess = ipc::connect_local_stream(&dir.socket()).unwrap();
    assert_eq!(read_response(excess).error.unwrap().code, "server_busy");
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
