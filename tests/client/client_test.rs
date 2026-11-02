//! Built-binary thin-client and cross-area suites.

#![cfg(unix)]

#[path = "../support/fixture_dir_test.rs"]
mod fixture_dirs;

// Separate instances retain the original suite's cleanup registry and support test identities.
#[allow(clippy::duplicate_mod)]
#[path = "../support/process_test.rs"]
pub mod client_mode_support;
#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;

mod client_mode {
    use super::client_mode_support as support;

    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
    use serde_json::Value;
    use support::{
        cleanup_test_base, client_shell_handshake, read_server_message, register_runtime_dir,
        register_spawned_bus_pid, stop_spawned_bus, unregister_spawned_bus_pid,
        wait_for_client_shell_bootstrap, wait_for_message_variant, wait_for_message_variants,
        wait_for_socket, wait_until, CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL,
        SERVER_MESSAGE_PANE_SURFACE, SERVER_MESSAGE_PANE_SURFACE_PATCH,
        SERVER_MESSAGE_SEMANTIC_NOTIFICATION, SERVER_MESSAGE_SERVER_SHUTDOWN,
    };

    fn unique_test_dir() -> PathBuf {
        crate::fixture_dirs::unique_test_dir("bus-client-test")
    }

    struct SpawnedBus {
        _master: Option<Box<dyn MasterPty + Send>>,
        child: Box<dyn Child + Send + Sync>,
    }

    impl SpawnedBus {
        fn close_master(&mut self) {
            drop(self._master.take());
        }
    }

    impl Drop for SpawnedBus {
        fn drop(&mut self) {
            let pid = self.child.process_id();
            stop_spawned_bus(self.child.as_mut(), self._master.take());
            unregister_spawned_bus_pid(pid);
        }
    }

    fn cleanup_spawned_bus(spawned: SpawnedBus, base: PathBuf) {
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
    ) -> SpawnedBus {
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
        cmd.env("BUS_DISABLE_SOUND", "1");
        cmd.env("XDG_STATE_HOME", runtime_dir.join("state"));
        cmd.env("XDG_CONFIG_HOME", config_home);
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
        support::configure_fixture_home(&mut cmd, &runtime_dir.join("home"));
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_bus_pid(child.process_id());
        drop(pair.slave);

        SpawnedBus {
            _master: Some(pair.master),
            child,
        }
    }

    fn spawn_server(
        config_home: &PathBuf,
        runtime_dir: &PathBuf,
        api_socket_path: &PathBuf,
    ) -> SpawnedBus {
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
    ) -> SpawnedBus {
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
        support::configure_fixture_home(&mut cmd, &runtime_dir.join("home"));
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_bus_pid(child.process_id());
        drop(pair.slave);

        SpawnedBus {
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
            "bus-dev"
        } else {
            "bus"
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

#[allow(clippy::duplicate_mod)]
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
    use std::time::{Duration, Instant};

    use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
    use serde_json::{json, Value};
    use support::{
        cleanup_test_base, client_shell_handshake, register_runtime_dir, register_spawned_bus_pid,
        stop_spawned_bus, unregister_spawned_bus_pid, wait_for_socket,
        CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL, SERVER_MESSAGE_ENDPOINT_CONTROL,
        SERVER_MESSAGE_PANE_SURFACE, SERVER_MESSAGE_PANE_SURFACE_PATCH,
    };

    fn unique_test_dir() -> PathBuf {
        crate::fixture_dirs::unique_test_dir("bus-cross-area-test")
    }

    struct SpawnedBus {
        _master: Option<Box<dyn MasterPty + Send>>,
        child: Box<dyn Child + Send + Sync>,
    }

    impl SpawnedBus {
        fn close_master(&mut self) {
            drop(self._master.take());
        }
    }

    impl Drop for SpawnedBus {
        fn drop(&mut self) {
            let pid = self.child.process_id();
            stop_spawned_bus(self.child.as_mut(), self._master.take());
            unregister_spawned_bus_pid(pid);
        }
    }

    fn cleanup_spawned_bus(spawned: SpawnedBus, base: PathBuf) {
        drop(spawned);
        cleanup_test_base(&base);
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn spawn_server(config_home: &Path, runtime_dir: &Path, api_socket_path: &Path) -> SpawnedBus {
        spawn_server_with_path(config_home, runtime_dir, api_socket_path, None)
    }

    fn spawn_server_with_path(
        config_home: &Path,
        runtime_dir: &Path,
        api_socket_path: &Path,
        path_override: Option<&Path>,
    ) -> SpawnedBus {
        fs::create_dir_all(config_home.join("bus")).unwrap();
        fs::create_dir_all(runtime_dir).unwrap();
        register_runtime_dir(runtime_dir);
        fs::write(config_home.join("bus/config.toml"), "onboarding = false\n").unwrap();

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
        support::configure_fixture_home(&mut cmd, &runtime_dir.join("home"));
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
        register_spawned_bus_pid(child.process_id());
        drop(pair.slave);

        SpawnedBus {
            _master: Some(pair.master),
            child,
        }
    }

    fn spawn_client_process(
        config_home: &Path,
        runtime_dir: &Path,
        api_socket_path: &Path,
    ) -> SpawnedBus {
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
        cmd.env("BUS_DISABLE_SOUND", "1");
        cmd.env("XDG_STATE_HOME", runtime_dir.join("state"));
        cmd.env("XDG_CONFIG_HOME", config_home);
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
        support::configure_fixture_home(&mut cmd, &runtime_dir.join("home"));
        cmd.env("HERDR_SOCKET_PATH", api_socket_path);
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_bus_pid(child.process_id());
        drop(pair.slave);

        SpawnedBus {
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

    // Preserve the detach test's polling cadence and budgets while retaining the
    // observations that distinguish process acquisition from screen detection.
    fn wait_for_detach_pane_value(
        socket_path: &Path,
        pane_id: &str,
        phase: &str,
        field: &str,
        expected: &str,
        timeout: Duration,
        poll_interval: Duration,
        launched_at: Instant,
    ) -> bool {
        let started = Instant::now();
        let deadline = started + timeout;
        let mut samples = 0;
        let mut max_api_time = Duration::ZERO;
        let mut last_response = Value::Null;
        let mut previous_state = Value::Null;
        let mut transitions = Vec::new();
        let mut matched = false;
        while Instant::now() < deadline {
            let request_started = Instant::now();
            last_response = send_json_request(
                socket_path,
                "detach_probe",
                "pane.get",
                json!({ "pane_id": pane_id }),
            );
            max_api_time = max_api_time.max(request_started.elapsed());
            samples += 1;
            let pane = &last_response["result"]["pane"];
            let state = json!({
                "agent": pane["agent"],
                "status": pane["agent_status"],
                "process_exited": pane["agent_process_exited"],
                "error": last_response["error"],
            });
            if state != previous_state {
                transitions.push(json!({
                    "elapsed_ms": started.elapsed().as_millis(),
                    "state": state,
                }));
                previous_state = state;
            }
            if pane[field].as_str() == Some(expected) {
                matched = true;
                break;
            }
            thread::sleep(poll_interval);
        }
        eprintln!(
            "detach_probe {}",
            json!({
                "phase": phase,
                "matched": matched,
                "elapsed_ms": started.elapsed().as_millis(),
                "since_launch_ms": launched_at.elapsed().as_millis(),
                "budget_ms": timeout.as_millis(),
                "samples": samples,
                "max_api_ms": max_api_time.as_millis(),
                "transitions": transitions,
                "last_response": last_response,
                "api_socket": socket_path,
            })
        );
        matched
    }

    fn detach_failure_snapshot(socket_path: &Path, pane_id: &str) -> Value {
        // A failing server must not turn diagnostic reads into an unbounded wait.
        let read = |source: &str| -> Result<Value, String> {
            let mut stream = UnixStream::connect(socket_path).map_err(|e| e.to_string())?;
            let timeout = Some(Duration::from_millis(500));
            stream
                .set_read_timeout(timeout)
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(timeout)
                .map_err(|e| e.to_string())?;
            writeln!(
                stream,
                "{}",
                json!({
                    "id": "detach_failure_snapshot",
                    "method": "pane.read",
                    "params": { "pane_id": pane_id, "source": source, "lines": 200 },
                })
            )
            .map_err(|e| e.to_string())?;
            let mut response = String::new();
            BufReader::new(stream)
                .read_line(&mut response)
                .map_err(|e| e.to_string())?;
            serde_json::from_str(&response).map_err(|e| e.to_string())
        };
        json!({ "recent": read("recent"), "visible": read("visible") })
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
