use super::*;
use interprocess::local_socket::traits::Listener as _;

#[test]
fn delayed_blocking_response_returns_receive_timeout() {
    let dir = crate::utils::test_temp::unique_temp_path("poll-timeout");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api.sock");
    let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut stream = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(&mut stream).read_line(&mut request).unwrap();
        ready_tx.send(request).unwrap();
        done_rx.recv().unwrap();
    });
    let client = ApiClient::for_target(ConnectionTarget::SocketPath(path));
    let result = client.request_value_with_timeout(
        &Request {
            id: "poll".into(),
            method: crate::protocol::api::schema::Method::AgentList(
                crate::protocol::api::schema::EmptyParams {},
            ),
        },
        Duration::from_millis(30),
    );
    let request = ready_rx.recv().unwrap();
    done_tx.send(()).unwrap();
    server.join().unwrap();
    assert!(request.contains("agent.list"));
    assert!(
        matches!(result, Err(ApiClientError::Io(ref error)) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)),
        "{result:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_timeout_einval_is_a_closed_peer_not_an_invalid_duration() {
    let dir = crate::utils::test_temp::unique_temp_path("timeout-einval");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api.sock");
    let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
    let peer = crate::platform::ipc::connect_local_stream(&path).unwrap();
    let stream = listener.accept().unwrap();
    stream
        .set_send_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_recv_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    drop(peer);
    assert_eq!(
        stream
            .set_send_timeout(Some(Duration::from_secs(5)))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EINVAL)
    );
    assert_eq!(
        stream
            .set_recv_timeout(Some(Duration::from_secs(4)))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EINVAL)
    );
    drop(stream);
    drop(listener);
    std::fs::remove_dir_all(dir).unwrap();
}
