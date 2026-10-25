use super::*;
use interprocess::local_socket::traits::Stream as _;
use serde_json::json;

#[test]
fn large_response_survives_coarse_poll_scheduling() {
    let dir = crate::utils::test_temp::unique_temp_path("bcl");
    storage_io::private_dir(&dir).unwrap();
    let (listener, socket) = bind_endpoint(&dir, SOCKET_NAME).unwrap();
    let path = dir.clone();
    let caller = thread::spawn(move || {
        request(
            &path,
            &Request {
                id: "large-response".into(),
                method: "room.history".into(),
                params: json!({"room":"1"}),
            },
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match listener.accept() {
            Ok(stream) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline);
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => panic!("{error}"),
        }
    };
    let mut client = Client::new(stream);
    assert!(client.respond(Response::success(
        "large-response",
        json!({"messages":[{"text":"x".repeat(2 * 1024 * 1024)}]}),
    )));
    let (commands, _receiver) = mpsc::sync_channel(1);
    let mut next = CONTROL_COMMAND_BIT;
    // Reproduce macOS timer coalescing and its small socket buffers: steady
    // transfer progress must not consume the three-second *idle* budget.
    while client.poll(&commands, &mut next, None) {
        thread::sleep(Duration::from_millis(20));
    }
    drop(client);
    let result = caller.join().unwrap();
    drop(socket);
    std::fs::remove_dir_all(dir).unwrap();
    let response = result.expect("a writable large response must not expire between chunks");
    assert!(response.ok);
    assert_eq!(
        response.result["messages"][0]["text"]
            .as_str()
            .unwrap()
            .len(),
        2 * 1024 * 1024
    );
}

#[test]
fn response_write_timeout_records_size_progress_and_request() {
    let dir = crate::utils::test_temp::unique_temp_path("bct");
    storage_io::private_dir(&dir).unwrap();
    let (listener, socket) = bind_endpoint(&dir, SOCKET_NAME).unwrap();
    let _peer = ipc::connect_local_stream(&dir.join(SOCKET_NAME)).unwrap();
    let mut client = Client::new(listener.accept().unwrap());
    client.request = Some(("history-write-timeout".into(), "room.history"));
    assert!(client.respond(Response::success(
        "history-write-timeout",
        json!({"secret":"private history"})
    )));
    let State::Writing { bytes, .. } = &client.state else {
        panic!("expected writing state")
    };
    let size = bytes.len();
    client.deadline = Instant::now() - Duration::from_millis(1);
    let (commands, _receiver) = mpsc::sync_channel(1);
    let mut next = CONTROL_COMMAND_BIT;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("info", || assert!(!client.poll(&commands, &mut next, None)));
    let log = capture.text();
    assert!(
        log.contains("bus.control.response.failed")
            && log.contains("write_timeout")
            && log.contains("history-write-timeout"),
        "{log}"
    );
    assert!(
        log.contains(&format!("response_bytes={size}")) && log.contains("written_bytes=0"),
        "{log}"
    );
    assert!(
        !log.contains("private history") && !log.contains("secret"),
        "{log}"
    );
    drop(client);
    drop(socket);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn response_progress_renews_idle_budget_but_cannot_extend_total_budget() {
    let dir = crate::utils::test_temp::unique_temp_path("bcb");
    storage_io::private_dir(&dir).unwrap();
    let (listener, socket) = bind_endpoint(&dir, SOCKET_NAME).unwrap();
    let mut peer = ipc::connect_local_stream(&dir.join(SOCKET_NAME)).unwrap();
    let mut client = Client::new(listener.accept().unwrap());
    assert!(client.respond(Response::success("bounded", json!("x".repeat(512 * 1024)))));
    let (commands, _receiver) = mpsc::sync_channel(1);
    let mut next = CONTROL_COMMAND_BIT;
    let idle = Instant::now() + Duration::from_millis(100);
    client.deadline = idle;
    assert!(client.poll(&commands, &mut next, None));
    assert!(
        client.deadline > idle,
        "successful writes renew the idle deadline"
    );
    let cap = Instant::now() + Duration::from_millis(200);
    let State::Writing { expires, .. } = &mut client.state else {
        panic!("response exceeds the unread socket buffer")
    };
    *expires = cap;
    // Drain the peer so the next poll must make transfer progress.
    peer.set_nonblocking(true).unwrap();
    let mut buffer = [0; 64 * 1024];
    while matches!(
        ipc::poll_local_stream_read_count(&mut peer, &mut buffer),
        Ok(ipc::LocalStreamReadCount::Data(_))
    ) {}
    assert!(client.poll(&commands, &mut next, None));
    assert!(
        client.deadline <= cap,
        "progress must not push the total deadline out"
    );
    let State::Writing { expires, .. } = &mut client.state else {
        panic!("response must still be incomplete")
    };
    *expires = Instant::now() - Duration::from_millis(1);
    client.deadline = *expires;
    assert!(
        !client.poll(&commands, &mut next, None),
        "the total budget is finite"
    );
    drop(client);
    drop(socket);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn truncated_response_reports_received_bytes_and_logs_no_history() {
    let dir = crate::utils::test_temp::unique_temp_path("bci");
    storage_io::private_dir(&dir).unwrap();
    let (listener, socket) = bind_endpoint(&dir, SOCKET_NAME).unwrap();
    let peer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok(stream) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(POLL_INTERVAL);
                }
                Err(error) => panic!("{error}"),
            }
        };
        let mut frame = Frame::default();
        while frame
            .poll(&mut stream, MAX_REQUEST_BYTES)
            .ok()
            .flatten()
            .is_none()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(POLL_INTERVAL);
        }
        use std::io::Write;
        stream.write_all(b"private history").unwrap();
    });
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut result = None;
    capture.run_filtered("info", || {
        result = Some(request(
            &dir,
            &Request {
                id: "history-read-incomplete".into(),
                method: "room.history".into(),
                params: serde_json::Value::Null,
            },
        ));
    });
    peer.join().unwrap();
    let error = result.unwrap().unwrap_err();
    assert!(
        error.starts_with("response_incomplete:") && error.contains("15 bytes"),
        "{error}"
    );
    let log = capture.text();
    assert!(
        log.contains("response_incomplete")
            && log.contains("history-read-incomplete")
            && log.contains("received_bytes=15"),
        "{log}"
    );
    assert!(!log.contains("private history"), "{log}");
    drop(socket);
    std::fs::remove_dir_all(dir).unwrap();
}
