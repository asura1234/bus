//! Reattach after `bus restart` when this TUI is still the same package build.
use std::path::Path;
use std::time::{Duration, Instant};

use crate::utils::restart_record::{self, BuildStamp, RestartLog, RestartMarker};

pub(crate) const RECONNECT_WAIT: Duration = Duration::from_secs(60);

pub(crate) fn restart_in_progress() -> Option<RestartMarker> {
    let root = crate::utils::env::bus_data_dir()?;
    restart_record::read_marker(&restart_record::marker_path(&root))
        .ok()
        .flatten()
}

/// A queued loss can arrive after the helper removed its marker. A complete
/// build record naming a different process still identifies that replacement.
pub(crate) fn restart_target(previous_pid: Option<u32>) -> Option<RestartMarker> {
    if let Some(marker) = restart_in_progress() {
        return Some(marker);
    }
    let root = crate::utils::env::bus_data_dir()?;
    let path = restart_record::build_path(&root);
    let stamp = restart_record::read_build_stamp(&path)?;
    if stamp.pid? == previous_pid? || !restart_record::valid_build_sha(&stamp.sha) {
        return None;
    }
    let launch = restart_record::read_recorded_launch(&path).ok()?;
    if !launch.session_known {
        return None;
    }
    Some(RestartMarker {
        binary: stamp.binary?,
        session_id: launch.session_id,
        explicit_data_dir: launch.explicit_data_dir?,
        data_dir: root.display().to_string(),
        dev: launch.dev?,
        yolo: launch.yolo?,
    })
}

/// Waits until `ready` is true, remembering the newest marker so a rollback's
/// reattach command names the binary that stayed up.
pub(crate) fn wait_for_restart(
    timeout: Duration,
    mut latest: RestartMarker,
    mut marker: impl FnMut() -> Option<RestartMarker>,
    mut ready: impl FnMut() -> bool,
) -> Result<RestartMarker, RestartMarker> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(seen) = marker() {
            latest = seen;
        }
        if ready() {
            return Ok(latest);
        }
        if Instant::now() >= deadline {
            return Err(latest);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The replacement is ready when its build record is this restart's binary and
/// a different process from the one this client attached to. The socket is
/// opened only after that, so a same-binary restart does not handshake with
/// the server that is still exiting. Incomplete process records are not ready.
pub(crate) fn replacement_server_ready(previous_pid: Option<u32>) -> bool {
    let Some(root) = crate::utils::env::bus_data_dir() else {
        return false;
    };
    let Some(stamp) = restart_record::read_build_stamp(&restart_record::build_path(&root)) else {
        return false;
    };
    if !restart_record::valid_build_sha(&stamp.sha) {
        return false;
    }
    if let Some(marker) = restart_in_progress() {
        if stamp.binary.as_deref() != Some(marker.binary.as_str()) {
            return false;
        }
    }
    match (previous_pid, stamp.pid) {
        (Some(previous), Some(pid)) if pid != previous => {}
        _ => return false,
    }
    crate::platform::ipc::connect_local_stream(&crate::utils::socket_paths::client_socket_path())
        .is_ok()
}

/// Client attachment and helper server health have independent outcomes. Append
/// the client failure without overwriting the helper's pending server notice.
pub(crate) fn record_attach_failure(
    previous: Option<&BuildStamp>,
    marker: &RestartMarker,
    reason: &str,
) -> String {
    let root = Path::new(&marker.data_dir);
    let observed = restart_record::read_build_stamp(&restart_record::build_path(root));
    tracing::warn!(
        reason, previous_server = ?previous, observed_server = ?observed,
        marker_binary = %marker.binary, "restart client attachment failed"
    );
    let record = RestartLog {
        at_ms: crate::utils::time::now_ms(),
        who: format!("client:{}", std::process::id()),
        old_sha: previous
            .map_or("unknown", |server| server.sha.as_str())
            .into(),
        new_sha: observed
            .as_ref()
            .map_or("unknown", |server| server.sha.as_str())
            .into(),
        old_binary: previous
            .and_then(|server| server.binary.clone())
            .unwrap_or_default(),
        new_binary: marker.binary.clone(),
        outcome: "client_attach_failed".into(),
        reason: reason.into(),
    };
    let message = restart_record::reattach_message(marker, reason);
    match restart_record::append_log(&restart_record::log_path(root), &record) {
        Ok(()) => message,
        Err(error) => {
            tracing::error!(%error, "could not record restart client attachment failure");
            format!("{message}\nCould not append restarts/log.jsonl: {error}")
        }
    }
}

/// SIGTERM the headless coordinator this restart started, and only that
/// process. A normal TUI does not write `restarts/coordinator.pid`.
pub(crate) fn step_aside_headless_coordinator(root: &Path) -> bool {
    let Some(record) = restart_record::read_live_pid(&restart_record::coordinator_pid_path(root))
    else {
        return false;
    };
    if !crate::platform::process_still_live(record.pid, record.birth) {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(record.pid as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !crate::platform::process_still_live(record.pid, record.birth) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_attachment_storage_failure_is_reported() {
        let root = crate::utils::test_temp::unique_temp_path("attach-record-error");
        std::fs::create_dir_all(restart_record::log_path(&root)).unwrap();
        let marker = RestartMarker {
            binary: "/new/bus".into(),
            session_id: None,
            explicit_data_dir: true,
            data_dir: root.display().to_string(),
            dev: true,
            yolo: true,
        };
        let message = record_attach_failure(None, &marker, "handshake rejected");
        assert!(message.contains("handshake rejected"), "{message}");
        assert!(
            message.contains("Could not append restarts/log.jsonl"),
            "{message}"
        );
        assert!(!restart_record::outcome_path(&root).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wait_keeps_the_newest_marker_when_the_socket_stays_down() {
        let mut seen = Some(RestartMarker {
            binary: "/old/bus".into(),
            session_id: Some("abc".into()),
            explicit_data_dir: false,
            data_dir: "/tmp/bus".into(),
            dev: true,
            yolo: false,
        });
        let result = wait_for_restart(
            Duration::from_millis(40),
            seen.clone().unwrap(),
            || seen.clone(),
            || false,
        );
        let latest = result.unwrap_err();
        assert_eq!(
            restart_record::reattach_command(&latest),
            "/old/bus --dev resume abc"
        );
        seen = Some(RestartMarker {
            binary: "/new/bus".into(),
            ..latest
        });
        let result = wait_for_restart(
            Duration::from_millis(30),
            seen.clone().unwrap(),
            || seen.clone(),
            || true,
        );
        assert_eq!(result.unwrap().binary, "/new/bus");
    }

    #[test]
    fn replacement_waits_for_a_new_server_pid_and_the_marker_binary() {
        let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
        let root = crate::utils::test_temp::unique_temp_path("rrdy");
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("c.sock");
        let saved = [
            ("BUS_DATA_DIR", std::env::var_os("BUS_DATA_DIR")),
            (
                crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
                std::env::var_os(crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR),
            ),
        ];
        std::env::set_var("BUS_DATA_DIR", &root);
        std::env::set_var(
            crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
            &socket,
        );
        struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
        let _restore = Restore(saved.into());
        let sha = "cd".repeat(20);
        let build = restart_record::build_path(&root);
        restart_record::write_json(
            &build,
            &serde_json::json!({ "sha": sha, "binary": "/tmp/bus", "pid": 11 }),
        )
        .unwrap();
        restart_record::write_json(
            &restart_record::marker_path(&root),
            &RestartMarker {
                binary: "/tmp/bus".into(),
                session_id: None,
                explicit_data_dir: true,
                data_dir: root.display().to_string(),
                dev: true,
                yolo: true,
            },
        )
        .unwrap();
        assert!(
            !replacement_server_ready(Some(11)),
            "the dying server is not the replacement"
        );
        let listener = crate::platform::ipc::bind_local_listener(&socket).unwrap();
        let accept = std::thread::spawn(move || {
            use interprocess::local_socket::traits::Listener as _;
            for _ in 0..4 {
                if listener.accept().is_err() {
                    break;
                }
            }
        });
        assert!(!replacement_server_ready(Some(11)));
        restart_record::write_json(
            &build,
            &serde_json::json!({ "sha": sha, "binary": "/tmp/other", "pid": 22 }),
        )
        .unwrap();
        assert!(
            !replacement_server_ready(Some(11)),
            "a different binary is not this restart"
        );
        restart_record::write_json(
            &build,
            &serde_json::json!({ "sha": sha, "binary": "/tmp/bus", "pid": 22,
                "dev": true, "yolo": true, "explicit_data_dir": true, "session_id": null }),
        )
        .unwrap();
        assert!(replacement_server_ready(Some(11)));
        assert!(
            !replacement_server_ready(None),
            "an unknown attached process cannot identify its replacement"
        );
        std::fs::remove_file(restart_record::marker_path(&root)).unwrap();
        let completed = restart_target(Some(11)).unwrap();
        assert_eq!(completed.binary, "/tmp/bus");
        assert!(completed.dev && completed.yolo && completed.explicit_data_dir);
        assert!(
            restart_target(Some(22)).is_none(),
            "ordinary shutdown has no replacement"
        );
        assert!(
            restart_target(None).is_none(),
            "the attached process must be known"
        );
        restart_record::write_json(
            &build,
            &serde_json::json!({ "sha": sha, "binary": "/tmp/bus" }),
        )
        .unwrap();
        assert!(
            !replacement_server_ready(Some(11)),
            "an incomplete server record is not ready"
        );
        assert!(restart_target(Some(11)).is_none());
        for _ in 0..4 {
            let _ = crate::platform::ipc::connect_local_stream(&socket);
        }
        let _ = accept.join();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
