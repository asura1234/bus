//! Timed publication of worker snapshots.
use super::{Arc, BusSnapshot, Mutex, Worker};
use crate::messaging::perf;

impl Worker {
    pub(super) fn publish_snapshot(&self, snapshots: &Mutex<Arc<BusSnapshot>>) {
        let snapshot = self.timed_snapshot();
        if let Ok(mut shared) = snapshots.lock() {
            *shared = snapshot;
        }
    }

    pub(super) fn timed_snapshot(&self) -> Arc<BusSnapshot> {
        let _timer = perf::SNAPSHOT.start();
        Arc::new(self.snapshot())
    }
}
