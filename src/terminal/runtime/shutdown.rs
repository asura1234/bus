use crate::terminal::runtime::TerminalRuntime;
use crate::utils::ids::PaneId;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tracing::info;
use tracing::warn;

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        // Abort detection task immediately and terminate the owned session.
        // The PTY actor shuts down before the process/session policy runs.
        if let Some(handle) = &self.detect_handle {
            handle.abort();
        }
        self.compression.abort();
        self.io.shutdown();
        if !self.preserve_processes_on_drop {
            shutdown_pane_processes(
                self.pane_id,
                self.child_pid.load(Ordering::Acquire),
                self.child_wait_completed.as_deref(),
            );
        }
    }
}

pub(super) fn process_alive_for_shutdown(
    pid: u32,
    child_pid: u32,
    child_wait_completed: bool,
    process_exists: impl FnOnce(u32) -> bool,
) -> bool {
    if pid == child_pid && child_wait_completed {
        return false;
    }
    process_exists(pid)
}

pub(super) fn wait_for_processes_to_exit(
    pids: &[u32],
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let child_wait_completed =
            child_wait_completed.is_some_and(|flag| flag.load(Ordering::Acquire));
        if pids.iter().all(|pid| {
            !process_alive_for_shutdown(
                *pid,
                child_pid,
                child_wait_completed,
                crate::platform::process_exists,
            )
        }) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub(super) fn shutdown_pane_processes(
    pane_id: PaneId,
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
) -> bool {
    if child_pid == 0 {
        return true;
    }

    let mut pids = crate::platform::session_processes(child_pid);
    if pids.is_empty() {
        pids.push(child_pid);
    }
    pids.sort_unstable();
    pids.dedup();

    for (signal, grace) in [
        (
            crate::platform::Signal::Hangup,
            std::time::Duration::from_millis(250),
        ),
        (
            crate::platform::Signal::Terminate,
            std::time::Duration::from_millis(250),
        ),
        (
            crate::platform::Signal::Kill,
            std::time::Duration::from_millis(250),
        ),
    ] {
        crate::platform::signal_processes(&pids, signal);
        if wait_for_processes_to_exit(&pids, child_pid, child_wait_completed, grace) {
            info!(
                pane = pane_id.raw(),
                pid = child_pid,
                ?signal,
                "pane session terminated"
            );
            return true;
        }
    }

    warn!(
        pane = pane_id.raw(),
        pid = child_pid,
        pids = ?pids,
        "pane session still alive after forced shutdown"
    );
    false
}

impl TerminalRuntime {
    pub fn shutdown(mut self) {
        if let Some(handle) = self.detect_handle.take() {
            handle.abort();
        }
        self.compression.abort();
        self.io.shutdown();
        shutdown_pane_processes(
            self.pane_id,
            self.child_pid.load(Ordering::Acquire),
            self.child_wait_completed.as_deref(),
        );
        self.preserve_processes_on_drop = true;
    }

    /// Checked shutdown for destructive API operations. Preserve the runtime
    /// for retry if a native process remains alive after the normal escalation.
    pub fn stop_session_for_close(&self) -> bool {
        let stopped = shutdown_pane_processes(
            self.pane_id,
            self.child_pid.load(Ordering::Acquire),
            self.child_wait_completed.as_deref(),
        );
        if stopped {
            // The subsequent normal close must not signal a recycled process id.
            self.child_pid.store(0, Ordering::Release);
        }
        stopped
    }
}
