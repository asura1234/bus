#[cfg(unix)]
use interprocess::local_socket::traits::Listener as _;

#[cfg(unix)]
fn unique_test_path(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("bus-{name}-{}-{nanos}", std::process::id()))
}

#[cfg(unix)]
fn local_stream_pair(name: &str) -> (LocalStream, LocalStream, std::path::PathBuf) {
    let path = unique_test_path(name);
    let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
    let client = crate::platform::ipc::connect_local_stream(&path).unwrap();
    let server = listener.accept().unwrap();
    (client, server, path)
}

#[test]
fn stop_wait_timeout_allows_slow_graceful_shutdown() {
    assert_eq!(STOP_WAIT_TIMEOUT, Duration::from_secs(15));
}

#[test]
fn stop_request_errors_wait_for_socket_state() {
    for kind in [
        std::io::ErrorKind::BrokenPipe,
        std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::UnexpectedEof,
        std::io::ErrorKind::NotConnected,
        std::io::ErrorKind::TimedOut,
        std::io::ErrorKind::WouldBlock,
    ] {
        let err = std::io::Error::from(kind);
        assert!(stop_request_error_allows_wait(&err), "{kind:?}");
    }
}

#[test]
fn stop_timeout_invalid_input_waits_for_socket_state() {
    let err = std::io::Error::from(std::io::ErrorKind::InvalidInput);

    assert!(stop_timeout_error_allows_wait(&err));
}

#[test]
fn stop_timeout_unsupported_waits_for_socket_state_only_on_windows() {
    let err = std::io::Error::from(std::io::ErrorKind::Unsupported);

    assert_eq!(stop_timeout_error_allows_wait(&err), cfg!(windows));
}

#[test]
fn socket_timeouts_are_never_zero_duration() {
    assert_eq!(socket_timeout_from_remaining(Duration::ZERO), None);
    assert_eq!(
        socket_timeout_from_remaining(Duration::from_nanos(1)),
        Some(MIN_SOCKET_TIMEOUT)
    );
    assert_eq!(
        socket_timeout_from_remaining(Duration::from_millis(10)),
        Some(Duration::from_millis(10))
    );
}

#[cfg(unix)]
#[test]
fn stop_request_empty_response_waits_for_socket_state() {
    let (client, server, _path) = local_stream_pair("stop-empty-response");
    let handle = std::thread::spawn(move || {
        let mut request = String::new();
        let _ = BufReader::new(server).read_line(&mut request);
        request
    });
    let request = serde_json::json!({
        "id": "cli:session:stop",
        "method": "server.stop",
        "params": {}
    });

    assert_eq!(
        send_stop_request(
            client,
            &request,
            Instant::now() + Duration::from_millis(100)
        )
        .unwrap(),
        None
    );
    assert!(handle.join().unwrap().contains("server.stop"));
}

#[cfg(unix)]
#[test]
fn stop_without_any_recorded_local_session_succeeds() {
    struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    let lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let bus_env = crate::utils::config::test_without_bus_env(&lock);
    let restore = RestoreEnv(
        [
            "HOME",
            "BUS_DEV",
            "BUS_LOG",
            "BUS_DEV_EXISTING_SERVER",
            "HERDR_SOCKET_PATH",
            "HERDR_CLIENT_SOCKET_PATH",
            "HERDR_CONFIG_PATH",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect(),
    );
    let home = std::env::temp_dir().join(format!(
        "bus-stop-empty-home-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);

    let result = crate::cli::run(&["stop".to_owned()]);

    drop(restore);
    drop(bus_env);
    drop(lock);
    std::fs::remove_dir_all(&home).unwrap();
    assert!(
        result.is_ok(),
        "stopping when no local server has ever existed should succeed: {result:?}"
    );
}

#[cfg(unix)]
#[test]
fn stop_waits_for_final_save_after_socket_close() {
    let path = unique_test_path("stop-save");
    let flag = path.with_extension("flag");
    let ready = path.with_extension("ready");
    let script = r#"
import os, socket, sys, time
path, flag, ready = sys.argv[1:]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.bind(path)
sock.listen(1)
open(ready, "w").close()
conn, _ = sock.accept()
conn.sendall(b'{"ok":true}\n')
conn.close()
sock.close()
try:
    os.unlink(path)
except FileNotFoundError:
    pass
time.sleep(0.25)
open(flag, "w").close()
"#;
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&path)
        .arg(&flag)
        .arg(&ready)
        .spawn()
        .expect("python3");
    let started = Instant::now();
    while !ready.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "python socket did not become ready"
        );
        if let Ok(Some(status)) = child.try_wait() {
            panic!("python exited before ready: {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Reap alongside the wait. A zombie stays visible to process_instance until
    // this parent collects it, and the helper only returns once that pid is gone.
    let waiter = std::thread::spawn(move || child.wait());
    let stopped = super::stop_socket_with_timeout(
        path.clone(),
        vec![path.clone()],
        Duration::from_secs(2),
        "server",
    );
    let status = waiter.join().expect("reap join").expect("python wait");
    assert!(status.success(), "python peer failed: {status}");
    stopped.expect("stop");
    assert!(
        flag.exists(),
        "stop returned before final session save"
    );
}
