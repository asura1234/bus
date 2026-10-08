//! Built-binary server lifecycle, sizing and client state suites.

#![cfg(unix)]

#[path = "../support/process_test.rs"]
pub mod detach_reattach_support;

#[path = "../support/process_test.rs"]
pub mod server_headless_support;

#[path = "../support/process_test.rs"]
pub mod multi_client_support;

mod detach_reattach {
    use super::detach_reattach_support as support;

    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
    use serde_json::Value;
    use support::{
        cleanup_test_base, client_shell_handshake, drain_messages, register_runtime_dir,
        register_spawned_herdr_pid, send_detach, unregister_spawned_herdr_pid, wait_for_disconnect,
        wait_for_socket, wait_until, CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL,
    };

    const CUSTOM_HEADLESS_SIZE_CONFIG: &str = r#"onboarding = false

[server]
headless_cols = 132
headless_rows = 41

[ui]
hide_tab_bar_when_single_tab = true
pane_scrollbars = false
"#;

    fn unique_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        PathBuf::from(format!(
            "/tmp/herdr-detach-test-{}-{nanos}",
            std::process::id()
        ))
    }

    struct SpawnedHerdr {
        _master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
    }

    impl Drop for SpawnedHerdr {
        fn drop(&mut self) {
            let pid = self.child.process_id();
            let _ = self.child.kill();

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
        fs::create_dir_all(config_home.join("herdr")).unwrap();
        fs::create_dir_all(runtime_dir).unwrap();
        register_runtime_dir(runtime_dir);
        fs::write(config_home.join("herdr/config.toml"), config).unwrap();

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
        cmd.env("HERDR_CONFIG_PATH", config_home.join("herdr/config.toml"));
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("HERDR_ENV");
        cmd.env_remove("BUS_DATA_DIR");
        cmd.env_remove("BUS_SESSION_ID");
        cmd.env_remove("HERDR_SESSION");

        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_herdr_pid(child.process_id());
        drop(pair.slave);

        SpawnedHerdr {
            _master: pair.master,
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

    fn workspace_create(socket_path: &PathBuf, label: &str) -> Value {
        send_json_request(
            socket_path,
            &format!(
                r#"{{"id":"workspace_create","method":"workspace.create","params":{{"label":"{label}"}}}}"#
            ),
        )
    }

    fn workspace_list(socket_path: &PathBuf) -> Value {
        send_json_request(
            socket_path,
            r#"{"id":"workspace_list","method":"workspace.list","params":{}}"#,
        )
    }

    fn pane_list(socket_path: &PathBuf, workspace_id: &str) -> Value {
        send_json_request(
            socket_path,
            &format!(
                r#"{{"id":"pane_list","method":"pane.list","params":{{"workspace_id":"{workspace_id}"}}}}"#
            ),
        )
    }

    fn pane_read_recent(socket_path: &PathBuf, pane_id: &str) -> Value {
        send_json_request(
            socket_path,
            &format!(
                r#"{{"id":"pane_read","method":"pane.read","params":{{"pane_id":"{pane_id}","source":"recent"}}}}"#
            ),
        )
    }

    fn pane_read_recent_text(socket_path: &PathBuf, pane_id: &str) -> String {
        pane_read_recent(socket_path, pane_id)["result"]["read"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn pane_send_text(socket_path: &PathBuf, pane_id: &str, text: &str) -> Value {
        send_json_request(
            socket_path,
            &format!(
                r#"{{"id":"pane_send_text","method":"pane.send_text","params":{{"pane_id":"{pane_id}","text":{}}}}}"#,
                serde_json::to_string(text).unwrap()
            ),
        )
    }

    fn parse_size_after_marker(text: &str, marker: &str) -> Option<(u16, u16)> {
        let lines: Vec<&str> = text.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !line.contains(marker) {
                continue;
            }
            for candidate in lines.iter().skip(idx + 1).take(6) {
                let mut parts = candidate.split_whitespace();
                let Some(rows) = parts.next().and_then(|part| part.parse::<u16>().ok()) else {
                    continue;
                };
                let Some(cols) = parts.next().and_then(|part| part.parse::<u16>().ok()) else {
                    continue;
                };
                if parts.next().is_none() {
                    return Some((rows, cols));
                }
            }
        }
        None
    }

    fn read_pane_tty_size_after_marker(
        socket_path: &PathBuf,
        pane_id: &str,
        marker: &str,
        timeout: Duration,
    ) -> (u16, u16) {
        pane_send_text(socket_path, pane_id, &format!("echo {marker}; stty size\n"));

        let deadline = Instant::now() + timeout;
        let mut last_text = String::new();
        while Instant::now() < deadline {
            last_text = pane_read_recent_text(socket_path, pane_id);
            if let Some(size) = parse_size_after_marker(&last_text, marker) {
                return size;
            }
            thread::sleep(Duration::from_millis(50));
        }

        panic!("did not observe tty size after marker {marker}. pane output:\n{last_text}");
    }

    fn workspace_id_by_label(response: &Value, label: &str) -> String {
        response["result"]["workspaces"]
            .as_array()
            .expect("workspace.list should return an array")
            .iter()
            .find(|workspace| workspace["label"] == label)
            .and_then(|workspace| workspace["workspace_id"].as_str())
            .expect("workspace with expected label should exist")
            .to_string()
    }

    fn first_pane_id(response: &Value) -> String {
        response["result"]["panes"]
            .as_array()
            .expect("pane.list should return an array")
            .first()
            .and_then(|pane| pane["pane_id"].as_str())
            .expect("pane.list should contain at least one pane")
            .to_string()
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------
    include!("reattach_test.rs");
    include!("headless_size_test.rs");
}

#[path = "multi_client_test.rs"]
mod multi_client;
#[path = "lifecycle_test.rs"]
mod server_headless;
