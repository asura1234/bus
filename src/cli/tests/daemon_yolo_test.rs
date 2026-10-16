#[test]
fn yolo_attach_requires_the_same_run_mode_as_the_server() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::utils::config::test_without_bus_env(&_guard);
    let dir = unique_test_dir("yolo-attach");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api.sock");
    std::env::set_var(crate::protocol::api::SOCKET_PATH_ENV_VAR, &path);
    for (requested, serving, want_ok) in [
        (false, true, false),
        (true, false, false),
        (false, false, true),
        (true, true, true),
    ] {
        if requested {
            std::env::set_var("BUS_YOLO", "1");
        } else {
            std::env::remove_var("BUS_YOLO");
        }
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let body = serde_json::json!({"id":"autodetect:server:status","result":{"type":"pong","version":"test","protocol":crate::protocol::wire::PROTOCOL_VERSION,"capabilities":{"endpoint_protocol_generation":crate::protocol::wire::handshake::ENDPOINT_PROTOCOL_GENERATION,"yolo":serving}}});
            writeln!(stream, "{body}").unwrap();
        });
        let result = validate_running_server_compatibility(false);
        handle.join().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            result.is_ok(),
            want_ok,
            "requested={requested}, serving={serving}: {result:?}"
        );
    }
    std::env::remove_var("BUS_YOLO");
    std::env::remove_var(crate::protocol::api::SOCKET_PATH_ENV_VAR);
    std::fs::remove_dir_all(dir).unwrap();
}
