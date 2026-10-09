use super::*;
use interprocess::local_socket::traits::Listener as _;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver};

fn local_stream_pair(name: &str) -> (LocalStream, LocalStream, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "bus-api-{name}-{}-{}.sock",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
    let client = crate::platform::ipc::connect_local_stream(&path).unwrap();
    let server = listener.accept().unwrap();
    (client, server, path)
}

fn spawn_connection(
    server: LocalStream,
) -> (Receiver<std::io::Result<()>>, std::thread::JoinHandle<()>) {
    let (done_tx, done_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (api_tx, _api_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = handle_connection(
            server,
            &api_tx,
            &EventHub::default(),
            &Arc::new(AtomicBool::new(true)),
        );
        done_tx.send(result).unwrap();
    });
    (done_rx, thread)
}

#[test]
fn windows_delayed_partial_initial_request_returns_pong() {
    let (mut client, server, path) = local_stream_pair("delayed-request");
    let (done_rx, server_thread) = spawn_connection(server);

    std::thread::sleep(Duration::from_millis(300));
    assert!(
        done_rx.try_recv().is_err(),
        "idle connected client must not be treated as closed"
    );

    client
        .write_all(br#"{"id":"delayed","method":"ping","params":{}}"#)
        .unwrap();
    client.flush().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        done_rx.try_recv().is_err(),
        "partial request must wait for its newline"
    );
    client.write_all(b"\n").unwrap();
    client.flush().unwrap();

    let mut response = String::new();
    BufReader::new(&mut client)
        .read_line(&mut response)
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], "delayed");
    assert_eq!(response["result"]["type"], "pong");

    done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    server_thread.join().unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn windows_disconnected_initial_request_returns_promptly() {
    let (client, server, path) = local_stream_pair("disconnected-request");
    let (done_rx, server_thread) = spawn_connection(server);

    drop(client);

    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("disconnected connection handler must finish promptly")
        .unwrap();
    server_thread.join().unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn windows_idle_initial_request_honors_timeout() {
    let (_client, mut server, path) = local_stream_pair("request-timeout");

    let err =
        read_initial_request_line_with_timeout(&mut server, Duration::from_millis(50)).unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    let _ = std::fs::remove_file(path);
}

#[test]
fn windows_initial_request_enforces_size_limit() {
    let (mut client, mut server, path) = local_stream_pair("request-size-limit");
    client.write_all(b"12345").unwrap();
    client.flush().unwrap();

    let err =
        read_initial_request_line_with_limits(&mut server, Duration::from_secs(1), 4).unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert_eq!(err.to_string(), "api request line is too large");
    let _ = std::fs::remove_file(path);
}

#[test]
fn windows_initial_request_rejects_invalid_utf8() {
    let (mut client, mut server, path) = local_stream_pair("request-invalid-utf8");
    client.write_all(&[0xff, b'\n']).unwrap();
    client.flush().unwrap();

    let err =
        read_initial_request_line_with_timeout(&mut server, Duration::from_secs(1)).unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    let _ = std::fs::remove_file(path);
}
