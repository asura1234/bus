//! A launch-local startup proof, independent of ongoing turn admission.
use super::rpc::Rpc;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const UNCHECKED: u8 = 0;
const CHECKING: u8 = 1;
const NATIVE_IDLE: u8 = 2;
const READY: u8 = 3;

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
        match self.0.load(Ordering::Acquire) {
            READY => return true,
            NATIVE_IDLE if frontend_idle => {
                self.0.store(READY, Ordering::Release);
                return true;
            }
            UNCHECKED if frontend_idle => {}
            _ => return false,
        }
        if self
            .0
            .compare_exchange(UNCHECKED, CHECKING, Ordering::AcqRel, Ordering::Acquire)
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
            let standalone =
                std::fs::read(directory.join("mode")).is_ok_and(|mode| mode == b"standalone");
            let ready = standalone
                || Rpc::connect(&socket, Instant::now() + Duration::from_millis(500))
                    .and_then(|mut rpc| rpc.startup_ready(&thread))
                    .unwrap_or(false);
            state.store(
                if ready { NATIVE_IDLE } else { UNCHECKED },
                Ordering::Release,
            );
        });
        false
    }
}
