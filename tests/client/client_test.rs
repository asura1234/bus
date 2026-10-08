//! Built-binary thin-client and cross-area suites.

#![cfg(unix)]

#[path = "../support/process_test.rs"]
pub mod client_mode_support;

mod client_mode {
    use super::client_mode_support as support;

    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
    use serde_json::Value;
    use support::{
        cleanup_test_base, client_shell_handshake, read_server_message, register_runtime_dir,
        register_spawned_herdr_pid, unregister_spawned_herdr_pid, wait_for_client_shell_bootstrap,
        wait_for_message_variant, wait_for_message_variants, wait_for_socket, wait_until,
        CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL, SERVER_MESSAGE_PANE_SURFACE,
        SERVER_MESSAGE_PANE_SURFACE_PATCH, SERVER_MESSAGE_SEMANTIC_NOTIFICATION,
        SERVER_MESSAGE_SERVER_SHUTDOWN,
    };

    fn unique_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        PathBuf::from(format!(
            "/tmp/herdr-client-test-{}-{nanos}",
            std::process::id()
        ))
    }

    struct SpawnedHerdr {
        _master: Option<Box<dyn MasterPty + Send>>,
        child: Box<dyn Child + Send + Sync>,
    }

    impl SpawnedHerdr {
        fn close_master(&mut self) {
            drop(self._master.take());
        }
    }

    impl Drop for SpawnedHerdr {
        fn drop(&mut self) {
            let pid = self.child.process_id();
            let _ = self.child.kill();
            self.close_master();

            if let Some(pid) = pid {
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    let mut status = 0;
                    let result =
                        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                    if result == pid as libc::pid_t || result == -1 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }

                unregister_spawned_herdr_pid(Some(pid));
            }
        }
    }

    fn cleanup_spawned_herdr(spawned: SpawnedHerdr, base: PathBuf) {
        drop(spawned);
        cleanup_test_base(&base);
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn spawn_client_process(
        config_home: &PathBuf,
        runtime_dir: &PathBuf,
        api_socket_path: &PathBuf,
    ) -> SpawnedHerdr {
        register_runtime_dir(runtime_dir);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
        cmd.arg("client");
        cmd.env("HERDR_DISABLE_SOUND", "1");
        cmd.env("XDG_STATE_HOME", runtime_dir.join("state"));
        cmd.env("XDG_CONFIG_HOME", config_home);
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_herdr_pid(child.process_id());
        drop(pair.slave);

        SpawnedHerdr {
            _master: Some(pair.master),
            child,
        }
    }

    fn spawn_server(
        config_home: &PathBuf,
        runtime_dir: &PathBuf,
        api_socket_path: &PathBuf,
    ) -> SpawnedHerdr {
        spawn_server_with_config(
            config_home,
            runtime_dir,
            api_socket_path,
            "onboarding = false\n",
        )
    }

    fn spawn_server_with_config(
        config_home: &PathBuf,
        runtime_dir: &PathBuf,
        api_socket_path: &PathBuf,
        config: &str,
    ) -> SpawnedHerdr {
        fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
        fs::create_dir_all(runtime_dir).unwrap();
        register_runtime_dir(runtime_dir);
        fs::write(config_home.join(app_dir_name()).join("config.toml"), config).unwrap();

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
        cmd.arg("server");
        cmd.env("XDG_CONFIG_HOME", config_home);
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_herdr_pid(child.process_id());
        drop(pair.slave);

        SpawnedHerdr {
            _master: Some(pair.master),
            child,
        }
    }

    fn ping_socket(socket_path: &PathBuf) -> String {
        let mut stream = UnixStream::connect(socket_path).expect("should connect to API socket");

        let request = r#"{"id":"1","method":"ping","params":{}}"#;
        writeln!(stream, "{}", request).unwrap();

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        response.trim().to_string()
    }

    fn send_json_request(socket_path: &PathBuf, request: &str) -> Value {
        let mut stream = UnixStream::connect(socket_path).expect("should connect to API socket");
        writeln!(stream, "{}", request).unwrap();

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).expect("response should be valid JSON")
    }

    fn first_pane_id_in_workspace(socket_path: &PathBuf, workspace_id: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let request = format!(
                r#"{{"id":"pane_list","method":"pane.list","params":{{"workspace_id":"{workspace_id}"}}}}"#
            );
            let panes = send_json_request(socket_path, &request);
            if let Some(pane_id) = panes["result"]["panes"]
                .as_array()
                .and_then(|panes| panes.first())
                .and_then(|pane| pane["pane_id"].as_str())
            {
                return pane_id.to_string();
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("pane.list did not return a pane for workspace {workspace_id} before timeout");
    }

    fn app_dir_name() -> &'static str {
        if cfg!(debug_assertions) {
            "herdr-dev"
        } else {
            "herdr"
        }
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------
    include!("startup_test.rs");
    include!("lifecycle_test.rs");
    include!("window_title_test.rs");
    include!("output_test.rs");
}

#[path = "../support/process_test.rs"]
pub mod cross_area_support;

mod cross_area {
    use super::cross_area_support as support;

    use std::fs;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
    use serde_json::{json, Value};
    use support::{
        cleanup_test_base, client_shell_handshake, register_runtime_dir,
        register_spawned_herdr_pid, unregister_spawned_herdr_pid, wait_for_socket,
        CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL, SERVER_MESSAGE_ENDPOINT_CONTROL,
        SERVER_MESSAGE_PANE_SURFACE, SERVER_MESSAGE_PANE_SURFACE_PATCH,
    };

    fn unique_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        PathBuf::from(format!(
            "/tmp/herdr-cross-area-test-{}-{nanos}",
            std::process::id()
        ))
    }

    struct SpawnedHerdr {
        _master: Option<Box<dyn MasterPty + Send>>,
        child: Box<dyn Child + Send + Sync>,
    }

    impl SpawnedHerdr {
        fn close_master(&mut self) {
            drop(self._master.take());
        }
    }

    impl Drop for SpawnedHerdr {
        fn drop(&mut self) {
            let pid = self.child.process_id();
            let _ = self.child.kill();
            self.close_master();

            if let Some(pid) = pid {
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    let mut status = 0;
                    let result =
                        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                    if result == pid as libc::pid_t || result == -1 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }

                unregister_spawned_herdr_pid(Some(pid));
            }
        }
    }

    fn cleanup_spawned_herdr(spawned: SpawnedHerdr, base: PathBuf) {
        drop(spawned);
        cleanup_test_base(&base);
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn spawn_server(
        config_home: &Path,
        runtime_dir: &Path,
        api_socket_path: &Path,
    ) -> SpawnedHerdr {
        spawn_server_with_path(config_home, runtime_dir, api_socket_path, None)
    }

    fn spawn_server_with_path(
        config_home: &Path,
        runtime_dir: &Path,
        api_socket_path: &Path,
        path_override: Option<&Path>,
    ) -> SpawnedHerdr {
        fs::create_dir_all(config_home.join("herdr")).unwrap();
        fs::create_dir_all(runtime_dir).unwrap();
        register_runtime_dir(runtime_dir);
        fs::write(
            config_home.join("herdr/config.toml"),
            "onboarding = false\n",
        )
        .unwrap();

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
        cmd.arg("server");
        cmd.env("XDG_STATE_HOME", runtime_dir.join("state"));
        cmd.env("XDG_CONFIG_HOME", config_home);
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");
        if let Some(path) = path_override {
            cmd.env("PATH", path);
        }

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_herdr_pid(child.process_id());
        drop(pair.slave);

        SpawnedHerdr {
            _master: Some(pair.master),
            child,
        }
    }

    fn spawn_client_process(
        config_home: &Path,
        runtime_dir: &Path,
        api_socket_path: &Path,
    ) -> SpawnedHerdr {
        register_runtime_dir(runtime_dir);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
        cmd.arg("client");
        cmd.env("HERDR_DISABLE_SOUND", "1");
        cmd.env("XDG_STATE_HOME", runtime_dir.join("state"));
        cmd.env("XDG_CONFIG_HOME", config_home);
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_herdr_pid(child.process_id());
        drop(pair.slave);

        SpawnedHerdr {
            _master: Some(pair.master),
            child,
        }
    }

    fn send_json_request(socket_path: &Path, id: &str, method: &str, params: Value) -> Value {
        let mut stream = UnixStream::connect(socket_path).expect("should connect to API socket");
        let request = json!({
            "id": id,
            "method": method,
            "params": params
        });
        writeln!(stream, "{}", request).unwrap();

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).expect("response should be valid JSON")
    }

    fn ping_socket(socket_path: &Path) -> String {
        let response = send_json_request(socket_path, "ping", "ping", json!({}));
        response.to_string()
    }

    fn workspace_create(socket_path: &Path, label: &str) -> Value {
        send_json_request(
            socket_path,
            "workspace_create",
            "workspace.create",
            json!({ "label": label, "focus": true }),
        )
    }

    fn workspace_list(socket_path: &Path) -> Value {
        send_json_request(socket_path, "workspace_list", "workspace.list", json!({}))
    }

    fn workspace_count(socket_path: &Path) -> usize {
        workspace_list(socket_path)["result"]["workspaces"]
            .as_array()
            .map(|workspaces| workspaces.len())
            .unwrap_or(0)
    }

    fn workspace_id_by_label(response: &Value, label: &str) -> String {
        response["result"]["workspaces"]
            .as_array()
            .expect("workspace.list should return workspaces array")
            .iter()
            .find(|workspace| workspace["label"] == label)
            .and_then(|workspace| workspace["workspace_id"].as_str())
            .expect("workspace with matching label should exist")
            .to_string()
    }

    fn wait_for_child_exit(child: &mut Box<dyn Child + Send + Sync>, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn pane_send_input(socket_path: &Path, pane_id: &str, text: &str) {
        let response = send_json_request(
            socket_path,
            "pane_send_input",
            "pane.send_input",
            json!({
                "pane_id": pane_id,
                "text": text,
                "keys": ["Enter"]
            }),
        );
        assert!(
            response.get("error").is_none(),
            "pane.send_input should succeed: {response}"
        );
    }

    fn pane_send_text(socket_path: &Path, pane_id: &str, text: &str) {
        let response = send_json_request(
            socket_path,
            "pane_send_text",
            "pane.send_text",
            json!({
                "pane_id": pane_id,
                "text": text
            }),
        );
        assert!(
            response.get("error").is_none(),
            "pane.send_text should succeed: {response}"
        );
    }

    fn pane_read_recent(socket_path: &Path, pane_id: &str) -> String {
        let response = send_json_request(
            socket_path,
            "pane_read",
            "pane.read",
            json!({
                "pane_id": pane_id,
                "source": "recent",
                "lines": 200
            }),
        );

        response["result"]["read"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn pane_read_recent_contains(
        socket_path: &Path,
        pane_id: &str,
        needle: &str,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let text = pane_read_recent(socket_path, pane_id);
            if text.contains(needle) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn pane_agent_status(socket_path: &Path, pane_id: &str) -> Option<String> {
        let response = send_json_request(
            socket_path,
            "pane_get",
            "pane.get",
            json!({ "pane_id": pane_id }),
        );
        response["result"]["pane"]["agent_status"]
            .as_str()
            .map(|status| status.to_string())
    }

    fn wait_for_agent_status(
        socket_path: &Path,
        pane_id: &str,
        expected: &str,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if pane_agent_status(socket_path, pane_id).as_deref() == Some(expected) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    // ---------------------------------------------------------------------------
    // Minimal protocol helpers (bincode v2 varint + framing)
    // ---------------------------------------------------------------------------

    fn encode_varint_u32(v: u32) -> Vec<u8> {
        if v < 251 {
            vec![v as u8]
        } else if v < 65_536 {
            let mut buf = vec![251u8];
            buf.extend_from_slice(&(v as u16).to_le_bytes());
            buf
        } else {
            let mut buf = vec![252u8];
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
    }

    fn frame_message(payload: &[u8]) -> Vec<u8> {
        let mut framed = (payload.len() as u32).to_le_bytes().to_vec();
        framed.extend_from_slice(payload);
        framed
    }

    fn decode_varint_u32(payload: &[u8], offset: usize) -> Result<(u32, usize), String> {
        if offset >= payload.len() {
            return Err("payload too short for varint".into());
        }
        let first = payload[offset];
        match first {
            0..=250 => Ok((first as u32, 1)),
            251 => {
                if offset + 3 > payload.len() {
                    return Err("payload too short for u16 varint".into());
                }
                let v = u16::from_le_bytes(
                    payload[offset + 1..offset + 3]
                        .try_into()
                        .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
                );
                Ok((v as u32, 3))
            }
            252 => {
                if offset + 5 > payload.len() {
                    return Err("payload too short for u32 varint".into());
                }
                let v = u32::from_le_bytes(
                    payload[offset + 1..offset + 5]
                        .try_into()
                        .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
                );
                Ok((v, 5))
            }
            _ => Err(format!("unsupported varint tag: {first}")),
        }
    }

    fn send_client_detach(stream: &mut UnixStream) {
        // ClientMessage::Detach = variant 4
        let payload = encode_varint_u32(4);
        stream
            .write_all(&frame_message(&payload))
            .expect("write detach");
        stream.flush().expect("flush detach");
    }

    fn is_timeout(err: &io::Error) -> bool {
        matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        )
    }

    fn read_server_variant(stream: &mut UnixStream, timeout: Duration) -> io::Result<u32> {
        stream.set_read_timeout(Some(timeout))?;
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zero-length payload",
            ));
        }
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload)?;
        decode_varint_u32(&payload, 0)
            .map(|(variant, _)| variant)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn wait_for_frame(stream: &mut UnixStream, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let slice = deadline.saturating_duration_since(Instant::now());
            match read_server_variant(stream, slice) {
                Ok(SERVER_MESSAGE_PANE_SURFACE | SERVER_MESSAGE_PANE_SURFACE_PATCH) => return true,
                Ok(SERVER_MESSAGE_ENDPOINT_CONTROL) => {}
                Ok(_) => {}
                Err(err) if is_timeout(&err) => {}
                Err(_) => return false,
            }
        }
        false
    }

    fn drain_server_messages(stream: &mut UnixStream, max_drain: Duration) {
        let deadline = Instant::now() + max_drain;
        while Instant::now() < deadline {
            match read_server_variant(stream, deadline.saturating_duration_since(Instant::now())) {
                Ok(_) => {}
                Err(err) if is_timeout(&err) => break,
                Err(_) => break,
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Cross-area tests
    // ---------------------------------------------------------------------------
    include!("persistence_test.rs");
    include!("shared_view_test.rs");
}
