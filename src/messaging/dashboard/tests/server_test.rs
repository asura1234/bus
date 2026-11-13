use super::*;
use crate::messaging::model::{BusState, Request};
use crate::messaging::storage::state_store::JsonStore;
use serde_json::{json, Value};

fn persisted_fixture(root: &Path) {
    let cases = super::fixtures::cases();
    let mut doc = serde_json::to_value(BusState::new()).unwrap();
    for (index, case) in cases.iter().enumerate().take(5) {
        doc["requests"][(100 + index).to_string()] = case["record"].clone();
    }
    let state: BusState = serde_json::from_value(doc).unwrap();
    JsonStore::new(root.join("state.json"))
        .save(&state)
        .unwrap();
    let archive: Request = serde_json::from_value(cases[19]["record"].clone()).unwrap();
    let line = serde_json::to_string(&json!({"request":archive})).unwrap();
    std::fs::write(
        root.join("history.jsonl"),
        format!("{line}\n{line}\n{{unfinished"),
    )
    .unwrap();
}

pub(super) fn files(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|item| {
            let path = item.unwrap().path();
            let data = std::fs::read(&path).unwrap();
            (path, data)
        })
        .collect();
    out.sort();
    out
}

// Catches dropping archived attempts, duplicate archive counts, writing migration
// or registry files, and leaking message/reply bodies through the HTTP boundary.
#[test]
fn data_route_reads_live_and_archived_requests_without_writes_or_message_content() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-data");
    std::fs::create_dir_all(&root).unwrap();
    persisted_fixture(&root);
    let before = files(&root);
    let response = response_for(&root, "GET", "/data");
    assert_eq!(response.status, "200 OK");
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["metrics"]["all"]["counts"]["attempts"], 6);
    assert_eq!(body["metrics"]["all"]["counts"]["external"], 1);
    assert!(body["timezone"].as_str().unwrap().contains("local"));
    let text = String::from_utf8(response.body).unwrap();
    assert!(!text.contains("redacted request"));
    assert!(!text.contains("redacted reply"));
    assert_eq!(files(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_or_corrupt_storage_serves_empty_data_without_creating_state() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-empty");
    let missing = response_for(&root, "GET", "/data");
    assert!(!root.exists());
    let body: Value = serde_json::from_slice(&missing.body).unwrap();
    assert!(body["metrics"].is_null());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("state.json"), "broken").unwrap();
    let before = files(&root);
    let corrupt = response_for(&root, "GET", "/data");
    let body: Value = serde_json::from_slice(&corrupt.body).unwrap();
    assert!(body["metrics"].is_null());
    assert_eq!(files(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn http_only_serves_known_get_routes_and_does_not_read_arbitrary_paths() {
    let root = Path::new("/nonexistent-dashboard-test");
    assert_eq!(
        response_for(root, "POST", "/data").status,
        "405 Method Not Allowed"
    );
    assert_eq!(
        response_for(root, "GET", "/../state.json").status,
        "404 Not Found"
    );
    let page = response_for(root, "GET", "/");
    assert_eq!(page.status, "200 OK");
    assert_eq!(page.content_type, "text/html; charset=utf-8");
}

#[test]
fn data_refresh_replays_newer_submit_journal_entries() {
    use crate::messaging::model::Provider;
    use crate::messaging::storage::state_store::JournalOp;
    let root = crate::utils::test_temp::unique_temp_path("dashboard-journal");
    std::fs::create_dir_all(&root).unwrap();
    let mut state = BusState::new();
    let room = state.create_room("journal").unwrap();
    let agent = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let store = JsonStore::new(root.join("state.json"));
    store.save(&state).unwrap();
    let first: Value = serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    assert_eq!(first["metrics"]["all"]["counts"]["attempts"], 0);
    store
        .journal(JournalOp::DraftText {
            room,
            text: "never expose this draft".into(),
        })
        .unwrap();
    store
        .journal(JournalOp::Submit {
            room,
            queued: true,
            now_ms: 1793577600000,
        })
        .unwrap();
    let before = files(&root);
    let refreshed: Value =
        serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    assert_eq!(refreshed["metrics"]["all"]["counts"]["attempts"], 1);
    assert_eq!(
        refreshed["metrics"]["providers"]["codex"]["counts"]["open"],
        1
    );
    assert_eq!(files(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn http_rejects_rebinding_cross_origin_and_duplicate_hosts() {
    let root = Path::new("/nonexistent-dashboard-test");
    let check =
        |request: &str| http_response(root, request.as_bytes(), "127.0.0.1:1234", "nonce").status;
    assert_eq!(
        check("GET /data HTTP/1.1\r\nHost: attacker.example\r\n\r\n"),
        "403 Forbidden"
    );
    assert_eq!(check("GET /data HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nOrigin: https://attacker.example\r\n\r\n"), "403 Forbidden");
    assert_eq!(
        check("GET /data HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nHost: 127.0.0.1:1234\r\n\r\n"),
        "403 Forbidden"
    );
    assert_eq!(
        check("GET /health HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n"),
        "200 OK"
    );
    assert_eq!(check("broken\r\n\r\n"), "400 Bad Request");
}

#[test]
fn idle_preconnect_does_not_delay_another_dashboard_request() {
    use std::net::TcpListener;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, ready) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        for n in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            if n == 0 {
                accepted.send(()).unwrap();
            }
            dispatch_connection(
                stream,
                "/nonexistent-dashboard-test".into(),
                address.to_string(),
                "nonce".into(),
            );
        }
    });
    let idle = TcpStream::connect(address).unwrap();
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut active = TcpStream::connect(address).unwrap();
    active
        .set_read_timeout(Some(Duration::from_millis(400)))
        .unwrap();
    active
        .write_all(format!("GET /health HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .unwrap();
    let mut response = String::new();
    let result = active.read_to_string(&mut response);
    drop(idle);
    server.join().unwrap();
    assert!(
        result.is_ok(),
        "an idle connection blocked a real request: {result:?}"
    );
    assert!(response.starts_with("HTTP/1.1 200 OK"));
}

#[test]
fn ordinary_browser_disconnects_and_header_timeouts_never_enter_warning_pipe() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        for kind in [
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::TimedOut,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::ConnectionReset,
        ] {
            log_http_error(&io::Error::new(kind, "ordinary browser connection"));
        }
    });
    assert!(!capture.text().contains("WARN"), "{}", capture.text());
}

#[test]
fn dashboard_health_names_the_build_that_embeds_the_page() {
    let response = http_response(
        Path::new("/nonexistent-dashboard-test"),
        b"GET /health HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
        "127.0.0.1:1234",
        "test-nonce",
    );
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["token"], "test-nonce");
    assert_eq!(body["build"], env!("BUS_BUILD_SHA"));
}

#[test]
fn dashboard_nonblocking_accept_preserves_fragmented_headers() {
    use std::net::TcpListener;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    client.write_all(b"GET /health HTTP/1.1\r\n").unwrap();
    // connect() can return before the server side queues the connection, so a
    // nonblocking accept may briefly see WouldBlock; serve_connections retries it too.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("dashboard listener never queued the connection: {error}"),
        }
    };
    dispatch_connection(
        stream,
        "/nonexistent-dashboard-test".into(),
        address.to_string(),
        "fragmented-nonce".into(),
    );
    let mut byte = [0u8];
    let pending = client.peek(&mut byte);
    assert!(
        matches!(&pending, Err(error) if matches!(
            error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        )),
        "HTTP worker closed or responded before the complete headers arrived: {pending:?}"
    );
    client
        .write_all(format!("Host: {address}\r\n\r\n").as_bytes())
        .unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response:?}");
    assert!(response.contains("fragmented-nonce"), "{response:?}");
}

#[test]
fn dashboard_nonblocking_accept_survives_connection_errors() {
    for error in [
        io::Error::from(io::ErrorKind::ConnectionAborted),
        io::Error::from(io::ErrorKind::ConnectionReset),
        io::Error::from(io::ErrorKind::BrokenPipe),
        io::Error::from_raw_os_error(libc::EMFILE),
    ] {
        let root = crate::utils::test_temp::unique_temp_path("dashboard-accept-errors");
        let service::Started::Owned(server) = service::start(&root).unwrap() else {
            panic!("test owns the dashboard listener")
        };
        server.listener.set_nonblocking(true).unwrap();
        let authority = server.listener.local_addr().unwrap().to_string();
        let endpoint = root.join("dashboard-runtime/endpoint.json");
        let detail = error.to_string();
        let mut failure = Some(error);
        let mut calls = 0;
        let result = serve_connections(&root, server, authority, |_| {
            calls += 1;
            if let Some(error) = failure.take() {
                return Err(error);
            }
            // A subsequent iteration must still honor cooperative retirement.
            let mut active: Value =
                serde_json::from_slice(&std::fs::read(&endpoint).unwrap()).unwrap();
            active["token"] = json!("retired-by-contract-test");
            std::fs::write(&endpoint, serde_json::to_vec(&active).unwrap()).unwrap();
            Err(io::Error::from(io::ErrorKind::Interrupted))
        });
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            result.is_ok(),
            "accept error stopped service: {detail}: {result:?}"
        );
        assert_eq!(calls, 2, "service did not resume accepting after {detail}");
    }
}
