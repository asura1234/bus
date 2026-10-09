use crate::terminal::emulator::PaneTerminal;
use crate::terminal::emulator::TerminalCompressionStep;
use crate::utils::ids::PaneId;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Notify;
use tracing::warn;

pub(super) const TERMINAL_COMPRESSION_IDLE: std::time::Duration =
    std::time::Duration::from_millis(250);

pub(super) const TERMINAL_COMPRESSION_STEP: std::time::Duration =
    std::time::Duration::from_millis(1);

pub(super) fn terminal_compression_permits() -> Arc<tokio::sync::Semaphore> {
    static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    PERMITS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
}

pub(super) fn spawn_blocking_with_compression_permit<T, F>(
    permit: tokio::sync::OwnedSemaphorePermit,
    operation: F,
) -> tokio::task::JoinHandle<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
}

// ---------------------------------------------------------------------------
// TerminalRuntime — PTY, parser, channels, background tasks
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(super) struct TerminalCompressionWake {
    pub(super) notify: Arc<Notify>,
    pub(super) generation: Arc<AtomicU64>,
}

impl TerminalCompressionWake {
    pub(super) fn wake(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.notify.notify_one();
    }
}

/// Drives libghostty-vt's caller-owned compression after terminal activity settles.
pub(super) struct TerminalCompressionTask {
    pub(super) wake: TerminalCompressionWake,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(super) completed_passes: Arc<AtomicU64>,
    pub(super) handle: tokio::task::AbortHandle,
}

impl Drop for TerminalCompressionTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl TerminalCompressionTask {
    pub(super) fn spawn(pane_id: PaneId, terminal: Arc<PaneTerminal>) -> Self {
        let wake = TerminalCompressionWake {
            notify: Arc::new(Notify::new()),
            generation: Arc::new(AtomicU64::new(0)),
        };
        let task_notify = wake.notify.clone();
        let task_generation = wake.generation.clone();
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let completed_passes = Arc::new(AtomicU64::new(0));
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let task_completed_passes = completed_passes.clone();
        let handle = tokio::spawn(async move {
            run_terminal_compression_task(
                pane_id,
                terminal,
                task_notify,
                task_generation,
                #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                task_completed_passes,
            )
            .await;
        })
        .abort_handle();
        Self {
            wake,
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            completed_passes,
            handle,
        }
    }

    pub(super) fn wake(&self) {
        self.wake.wake();
    }

    pub(super) fn notifier(&self) -> TerminalCompressionWake {
        self.wake.clone()
    }

    pub(super) fn abort(&self) {
        self.handle.abort();
    }

    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(super) fn completed_passes(&self) -> u64 {
        self.completed_passes.load(Ordering::Acquire)
    }
}

pub(super) async fn run_terminal_compression_task(
    pane_id: PaneId,
    terminal: Arc<PaneTerminal>,
    notify: Arc<Notify>,
    generation: Arc<AtomicU64>,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))] completed_passes: Arc<
        AtomicU64,
    >,
) {
    let mut observed_generation = generation.load(Ordering::Acquire);
    let mut activity = loop {
        match terminal.try_compression_activity() {
            Ok(Some(activity)) => break activity,
            Ok(None) => tokio::time::sleep(TERMINAL_COMPRESSION_IDLE).await,
            Err(err) => {
                warn!(pane = pane_id.raw(), err = %err, "failed to read terminal compression activity");
                return;
            }
        }
    };

    'schedule: loop {
        loop {
            tokio::time::sleep(TERMINAL_COMPRESSION_IDLE).await;
            let current = match terminal.try_compression_activity() {
                Ok(Some(current)) => current,
                Ok(None) => continue,
                Err(err) => {
                    warn!(pane = pane_id.raw(), err = %err, "failed to read terminal compression activity");
                    return;
                }
            };
            let current_generation = generation.load(Ordering::Acquire);
            if activity == current && observed_generation == current_generation {
                break;
            }
            activity = current;
            observed_generation = current_generation;
        }

        loop {
            let current_generation = generation.load(Ordering::Acquire);
            if observed_generation != current_generation {
                observed_generation = current_generation;
                continue 'schedule;
            }

            let permit = match terminal_compression_permits().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let current_generation = generation.load(Ordering::Acquire);
            if observed_generation != current_generation {
                observed_generation = current_generation;
                continue 'schedule;
            }

            let terminal_for_step = terminal.clone();
            let step = spawn_blocking_with_compression_permit(permit, move || {
                terminal_for_step.try_compress_incremental_if_activity(activity)
            })
            .await;
            let step = match step {
                Ok(Ok(step)) => step,
                Ok(Err(err)) => {
                    warn!(pane = pane_id.raw(), err = %err, "failed to compress terminal scrollback");
                    return;
                }
                Err(err) => {
                    warn!(pane = pane_id.raw(), err = %err, "terminal compression worker failed");
                    return;
                }
            };

            match step {
                TerminalCompressionStep::Busy => continue 'schedule,
                TerminalCompressionStep::ActivityChanged(current) => {
                    activity = current;
                    observed_generation = generation.load(Ordering::Acquire);
                    continue 'schedule;
                }
                TerminalCompressionStep::Compressed(
                    crate::terminal::vt::TerminalCompressionResult::Unsupported,
                ) => return,
                TerminalCompressionStep::Compressed(
                    crate::terminal::vt::TerminalCompressionResult::Pending,
                ) => tokio::time::sleep(TERMINAL_COMPRESSION_STEP).await,
                TerminalCompressionStep::Compressed(
                    crate::terminal::vt::TerminalCompressionResult::Complete,
                ) => {
                    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                    completed_passes.fetch_add(1, Ordering::Release);
                    loop {
                        notify.notified().await;
                        let current_generation = generation.load(Ordering::Acquire);
                        if observed_generation != current_generation {
                            observed_generation = current_generation;
                            continue 'schedule;
                        }
                    }
                }
            }
        }
    }
}
