#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::platform::PaneProcessTree;
use crate::platform::Signal;
use crate::terminal::runtime::TerminalRuntime;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use crate::utils::ids::PaneId;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const ESCALATION: [Signal; 3] = [Signal::Hangup, Signal::Terminate, Signal::Kill];
const GRACE: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(20);

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        if let Some(handle) = &self.detect_handle {
            handle.abort();
        }
        self.compression.abort();
        if self.preserve_processes_on_drop {
            self.io.shutdown();
        } else {
            self.shutdown_owned_processes(true);
        }
    }
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "linux"))))]
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn wait_for_processes_to_exit(
    pids: &[u32],
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
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
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn shutdown_pane_processes(
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
    info!(event = "bus.pane.shutdown", pane = pane_id.raw(), session_id = child_pid,
        pids = ?pids, "Stopping the owned PTY session, including surviving children");

    for signal in ESCALATION {
        crate::platform::signal_processes(&pids, signal);
        if wait_for_processes_to_exit(&pids, child_pid, child_wait_completed, GRACE) {
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl TerminalRuntime {
    fn child_reaped(&self) -> bool {
        self.child_wait_completed
            .as_deref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    fn shutdown_owned_processes(&self, close_io: bool) -> bool {
        let child_pid = self.child_pid.load(Ordering::Acquire);
        if child_pid == 0 {
            if close_io {
                self.io.shutdown();
            }
            return true;
        }

        let mut tree = self
            .shutdown_tree
            .take()
            .unwrap_or_else(|| PaneProcessTree::new(child_pid, self.child_instance));
        tree.begin_attempt();
        // Capture ancestry before closing the PTY can make a provider exit
        // and reparent descendants that have already changed sessions.
        tree.refresh(self.child_reaped());
        if close_io {
            self.io.shutdown();
        }
        let pane = self.pane_id.raw();
        info!(event = "bus.pane.shutdown", pane, session_id = child_pid,
            pids = ?tree.pids(), "Stopping the owned PTY session, including surviving children");

        for signal in ESCALATION {
            tree.refresh(self.child_reaped());
            tree.signal(signal, self.child_reaped());
            let deadline = Instant::now() + GRACE;
            loop {
                let empty = tree.is_empty(self.child_reaped());
                if empty || signal == Signal::Kill {
                    // A fork during a grace window can outlive its parent.
                    // During the last window, kill newly found processes too.
                    tree.refresh(self.child_reaped());
                    if tree.is_empty(self.child_reaped()) {
                        info!(pane, pid = child_pid, ?signal, "pane session terminated");
                        return true;
                    }
                    if signal == Signal::Kill {
                        tree.signal(signal, self.child_reaped());
                    }
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                std::thread::sleep(POLL.min(remaining));
            }
        }

        let survivors = tree.survivors(self.child_reaped());
        if survivors.is_empty() {
            return true;
        }
        warn!(
            pane,
            pid = child_pid,
            ?survivors,
            "pane session still alive after forced shutdown"
        );
        self.shutdown_tree.set(Some(tree));
        false
    }
}

impl TerminalRuntime {
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn shutdown_owned_processes(&self, close_io: bool) -> bool {
        if close_io {
            self.io.shutdown();
        }
        shutdown_pane_processes(
            self.pane_id,
            self.child_pid.load(Ordering::Acquire),
            self.child_wait_completed.as_deref(),
        )
    }

    pub fn shutdown(mut self) {
        if let Some(handle) = self.detect_handle.take() {
            handle.abort();
        }
        self.compression.abort();
        self.shutdown_owned_processes(true);
        self.preserve_processes_on_drop = true;
    }

    /// Checked shutdown for destructive API operations. Preserve the runtime
    /// for retry if a native process remains alive after the normal escalation.
    pub fn stop_session_for_close(&self) -> bool {
        let stopped = self.shutdown_owned_processes(false);
        if stopped {
            // The subsequent normal close must not signal a recycled process id.
            self.child_pid.store(0, Ordering::Release);
        }
        stopped
    }
}
