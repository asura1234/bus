//! A launch-local startup proof, independent of ongoing turn admission.
use super::{
    is_standalone,
    rpc::{Rpc, StartupProbe},
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

// Zero is the initial state. Probe outcomes occupy the values above READY.
const CHECKING: u8 = 1;
const READY: u8 = 2;
const NATIVE_IDLE: u8 = StartupProbe::Idle as u8;

#[derive(Default)]
pub(super) struct Readiness(Arc<AtomicU8>);

impl Readiness {
    #[cfg(test)]
    pub(super) fn confirmed() -> Self {
        Self(Arc::new(AtomicU8::new(READY)))
    }

    pub(super) fn ready(
        &self,
        socket: &Path,
        directory: &Path,
        thread: &str,
        frontend_idle: bool,
    ) -> bool {
        let previous = self.0.load(Ordering::Acquire);
        match previous {
            READY => return true,
            NATIVE_IDLE if frontend_idle => {
                self.0.store(READY, Ordering::Release);
                return true;
            }
            CHECKING | NATIVE_IDLE => return false,
            _ if !frontend_idle => return false,
            _ => {}
        }
        if self
            .0
            .compare_exchange(previous, CHECKING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let (socket, directory, thread, state) = (
            socket.to_owned(),
            directory.to_owned(),
            thread.to_owned(),
            self.0.clone(),
        );
        // Never hold the pane's input mutex or wait on the App dispatch thread.
        // A replacement launch gets a fresh state; this worker cannot bless it.
        std::thread::spawn(move || {
            let outcome = if is_standalone(&directory) {
                StartupProbe::Idle
            } else {
                Rpc::probe_startup(
                    &socket,
                    Instant::now() + Duration::from_millis(500),
                    &thread,
                )
            };
            if outcome as u8 != previous && !matches!(outcome, StartupProbe::Idle) {
                tracing::info!(
                    event = "bus.codex.startup_probe",
                    runtime = ?directory.file_name(),
                    outcome = ?outcome,
                    "Codex native startup probe is waiting"
                );
            }
            state.store(outcome as u8, Ordering::Release);
        });
        false
    }
}
