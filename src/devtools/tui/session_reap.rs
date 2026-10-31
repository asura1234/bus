//! Kill a scratch Bus when its test is gone. The host watches one pid and
//! signals every process still attached to the session directory.
use super::{pids_holding, pids_owned, process_alive, SessionPaths};
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn kill_pid(pid: u32) {
    if pid <= 1 || pid == std::process::id() {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    terminate(pid);
}

/// Ends this process without running destructors. The reap watcher uses it
/// after the scratch children are already signaled, so a dead test cannot
/// leave the detached host behind.
pub(super) fn kill_self() -> ! {
    #[cfg(unix)]
    unsafe {
        libc::kill(std::process::id() as libc::pid_t, libc::SIGKILL);
    }
    std::process::abort();
}

/// SIGTERM, then SIGKILL whatever is still alive after `grace`.
pub(super) fn reap_pids(pids: &[u32], grace: Duration) {
    let pids: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|pid| *pid > 1 && *pid != std::process::id())
        .collect();
    if pids.is_empty() {
        return;
    }
    for pid in &pids {
        terminate(*pid);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && pids.iter().any(|pid| process_alive(*pid)) {
        std::thread::sleep(Duration::from_millis(20));
    }
    for pid in &pids {
        if process_alive(*pid) {
            kill_pid(*pid);
        }
    }
}

/// Token holders plus anything else with a file open under `dir`.
pub(super) fn session_pids(dir: &Path, token: &str) -> Vec<u32> {
    let mut pids = if token.is_empty() {
        Vec::new()
    } else {
        pids_owned(dir, token)
    };
    for pid in pids_holding(dir) {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

/// Kills the host, the scratch Bus, and every process still attached to `dir`.
pub(super) fn reap_session_tree(
    dir: &Path,
    token: &str,
    host_pid: Option<u32>,
    bus_pid: Option<u32>,
) {
    let mut pids = session_pids(dir, token);
    for pid in [host_pid, bus_pid].into_iter().flatten() {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    reap_pids(&pids, Duration::from_millis(200));
}

/// Blocks until `observer` is gone, then kills this session's processes.
/// `observer` is the test process named by [`super::REAP_WITH_ENV`]. A zombie still
/// counts as alive, which matches `kill(pid, 0)` until the runner waits it.
pub(super) fn watch_until_observer_exits(observer: u32, paths: SessionPaths, token: String) {
    while process_alive(observer) {
        std::thread::sleep(Duration::from_millis(200));
    }
    let info = paths.read_info();
    reap_session_tree(
        &paths.dir,
        &token,
        info.as_ref().map(|info| info.host_pid),
        info.as_ref().and_then(|info| info.bus_pid),
    );
    // Test sessions use a private root (`/tmp/bti-…`). Processes whose cwd is
    // that root, rather than the session directory, still show up here. This
    // watcher runs only when a test set [`super::REAP_WITH_ENV`].
    if let Some(root) = paths.dir.parent() {
        reap_pids(&pids_holding(root), Duration::from_millis(50));
    }
}

pub(super) fn terminate(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}
