//! Reattach after `bus restart` when this TUI is still the same package build.
use std::path::Path;
use std::time::{Duration, Instant};

use crate::utils::restart_record::{self, RestartMarker};

pub(crate) const RECONNECT_WAIT: Duration = Duration::from_secs(60);

pub(crate) fn restart_in_progress() -> Option<RestartMarker> {
    let root = crate::utils::env::bus_data_dir()?;
    restart_record::read_marker(&restart_record::marker_path(&root))
        .ok()
        .flatten()
}

/// Waits until `ready` is true, remembering the newest marker so a rollback's
/// reattach command names the binary that stayed up.
pub(crate) fn wait_for_restart(
    timeout: Duration,
    mut marker: impl FnMut() -> Option<RestartMarker>,
    mut ready: impl FnMut() -> bool,
) -> Result<RestartMarker, RestartMarker> {
    let mut latest = marker().unwrap_or_else(|| RestartMarker {
        binary: "bus".into(),
        session_id: None,
        explicit_data_dir: false,
        data_dir: String::new(),
        dev: false,
        yolo: false,
    });
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

pub(crate) fn client_socket_ready() -> bool {
    if crate::platform::ipc::connect_local_stream(&crate::utils::socket_paths::client_socket_path())
        .is_err()
    {
        return false;
    }
    let Some(root) = crate::utils::env::bus_data_dir() else {
        return false;
    };
    restart_record::read_build_sha(&restart_record::build_path(&root))
        .is_some_and(|sha| restart_record::valid_build_sha(&sha))
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
    fn wait_keeps_the_newest_marker_when_the_socket_stays_down() {
        let mut seen = Some(RestartMarker {
            binary: "/old/bus".into(),
            session_id: Some("abc".into()),
            explicit_data_dir: false,
            data_dir: "/tmp/bus".into(),
            dev: true,
            yolo: false,
        });
        let result = wait_for_restart(Duration::from_millis(40), || seen.clone(), || false);
        let latest = result.unwrap_err();
        assert_eq!(
            restart_record::reattach_command(&latest),
            "/old/bus --dev resume abc"
        );
        seen = Some(RestartMarker {
            binary: "/new/bus".into(),
            ..latest
        });
        let result = wait_for_restart(Duration::from_millis(30), || seen.clone(), || true);
        assert_eq!(result.unwrap().binary, "/new/bus");
    }
}
