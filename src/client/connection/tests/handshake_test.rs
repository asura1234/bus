use super::*;
use crate::utils::restart_record;
use interprocess::local_socket::traits::Listener as _;

#[test]
fn attached_identity_is_read_after_the_server_welcome() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let root = crate::utils::test_temp::unique_temp_path("hs");
    std::fs::create_dir_all(&root).unwrap();
    struct Restore {
        root: std::path::PathBuf,
        value: Option<std::ffi::OsString>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.value.take() {
                Some(value) => std::env::set_var("BUS_DATA_DIR", value),
                None => std::env::remove_var("BUS_DATA_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    let _restore = Restore {
        root: root.clone(),
        value: std::env::var_os("BUS_DATA_DIR"),
    };
    std::env::set_var("BUS_DATA_DIR", &root);
    let sha = "ab".repeat(20);
    restart_record::write_json(
        &restart_record::build_path(&root),
        &serde_json::json!({ "sha": sha, "binary": "/old/bus", "pid": 11 }),
    )
    .unwrap();
    let socket = root.join("c.sock");
    let listener = crate::platform::ipc::bind_local_listener(&socket).unwrap();
    let serving_root = root.clone();
    let (handshake_done, wait_for_handshake) = std::sync::mpsc::channel();
    let serving = std::thread::spawn(move || {
        let mut peer = listener.accept().unwrap();
        set_handshake_recv_timeout(&peer, Some(Duration::from_secs(5)), "test peer timeout")
            .unwrap();
        let hello: ClientMessage = wire::read_message(&mut peer, MAX_FRAME_SIZE).unwrap();
        assert!(
            matches!(hello, ClientMessage::EndpointControl { kind, .. } if kind == ENDPOINT_HELLO_KIND)
        );
        // Startup publishes its serving identity after bind and before it can welcome clients.
        restart_record::write_json(
            &restart_record::build_path(&serving_root),
            &serde_json::json!({ "sha": sha, "binary": "/new/bus", "pid": 22 }),
        )
        .unwrap();
        let welcome = EndpointServerWelcome::compatible(Vec::new());
        wire::write_message(
            &mut peer,
            &ServerMessage::EndpointControl {
                kind: ENDPOINT_WELCOME_KIND.into(),
                data: serde_json::to_string(&welcome).unwrap(),
            },
        )
        .unwrap();
        // Keep the peer connected while the client clears its read timeout.
        // macOS returns EINVAL for that socket option after peer closure.
        wait_for_handshake
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
    });
    let mut stream = crate::platform::ipc::connect_local_stream(&socket).unwrap();
    let attached = do_handshake(
        &mut stream,
        8,
        16,
        true,
        crate::protocol::wire::ClientSurfaceSize { cols: 80, rows: 30 },
        false,
        false,
    );
    let _ = handshake_done.send(());
    serving.join().unwrap();
    let server = attached.unwrap().server.unwrap();
    assert_eq!(
        server.pid,
        Some(22),
        "cache the server that sent the welcome, not the stale startup stamp"
    );
    assert_eq!(server.binary.as_deref(), Some("/new/bus"));
}
