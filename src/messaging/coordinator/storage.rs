//! Durable storage pause and recovery for the single coordinator.
use super::{mpsc, BusEvent, BusState, Duration, Worker};
use crate::messaging::storage::state_store::StoreError;
use std::sync::Arc;
use std::time::Instant;

pub(super) type CoordinatorLease = crate::platform::fs::FileLock;

const STORAGE_RETRY_MIN: Duration = Duration::from_secs(1);
const STORAGE_RETRY_MAX: Duration = Duration::from_secs(30);
pub(super) const STORAGE_RETRYING: &str = "Storage paused; retrying";
pub(super) const STORAGE_NEEDS_REPAIR: &str = "Storage paused; durable state needs inspection";

pub(super) enum StoragePause {
    Retrying {
        next_retry: Instant,
        delay: Duration,
    },
    NeedsRepair,
}

impl Worker {
    pub(super) fn apply_poll(&mut self, state: BusState) -> Result<(), String> {
        if state.durable_poll_change_from(&self.state) {
            self.save(state)
        } else {
            // Keep the latest observation boundary for a later submission.
            // The submission save persists it before contacting the provider.
            self.state = state;
            Ok(())
        }
    }

    /// A callback that changes nothing must not fold an already durable draft
    /// journal into state.json. Pending batch changes still commit before ack.
    pub(super) fn save_callback_state(&mut self, state: BusState) -> Result<(), String> {
        if state != self.state {
            self.save(state)?;
        }
        Ok(())
    }

    pub(super) fn storage_notice(&self) -> &'static str {
        match self.storage_pause.as_ref() {
            Some(StoragePause::NeedsRepair) => STORAGE_NEEDS_REPAIR,
            _ => STORAGE_RETRYING,
        }
    }

    fn log_storage_error(error: &StoreError, event: &'static str) {
        let (stage, raw_os_error, bytes) = error.diagnostic();
        tracing::error!(event, stage, raw_os_error = ?raw_os_error, bytes, "Bus state storage failed");
    }

    pub(super) fn pause_storage(&mut self, error: &StoreError) {
        Self::log_storage_error(error, "bus.storage.failed");
        self.storage_pause = Some(if error.retryable_io() {
            StoragePause::Retrying {
                next_retry: Instant::now() + STORAGE_RETRY_MIN,
                delay: STORAGE_RETRY_MIN,
            }
        } else {
            StoragePause::NeedsRepair
        });
        self.error = Some(self.storage_notice().into());
    }

    fn defer_storage_retry(&mut self, now: Instant) {
        if let Some(StoragePause::Retrying { next_retry, delay }) = &mut self.storage_pause {
            *delay = (*delay * 2).min(STORAGE_RETRY_MAX);
            *next_retry = now + *delay;
        }
    }

    fn require_storage_repair(&mut self, reason: &'static str) {
        tracing::error!(
            event = "bus.storage.repair_required",
            reason,
            "Bus state differs from last committed state"
        );
        self.storage_pause = Some(StoragePause::NeedsRepair);
        self.error = Some(STORAGE_NEEDS_REPAIR.into());
    }

    pub(super) fn retry_storage(&mut self, now: Instant, events: &mpsc::Sender<BusEvent>) {
        let Some(StoragePause::Retrying { next_retry, .. }) = &self.storage_pause else {
            return;
        };
        if now < *next_retry {
            return;
        }
        // The archive beside state.json is append-only and only this
        // coordinator writes it, so only state.json can have diverged.
        let history = Arc::clone(self.durable_state.history());
        match self
            .store
            .load_base()
            .map(|durable| durable.map(|durable| durable.with_history(history)))
        {
            // Compare with the exact last commit: status-only polls and journal
            // entries may have advanced the in-memory state since that write.
            Ok(Some(durable)) if durable == self.durable_state => {}
            Ok(_) => {
                self.require_storage_repair("divergent_or_missing");
                return;
            }
            Err(error) if error.retryable_io() => {
                Self::log_storage_error(&error, "bus.storage.retry_failed");
                self.defer_storage_retry(now);
                return;
            }
            Err(error) => {
                Self::log_storage_error(&error, "bus.storage.validation_failed");
                self.require_storage_repair("unreadable");
                return;
            }
        }
        if let Err(error) = self.store.save(&self.state) {
            Self::log_storage_error(&error, "bus.storage.retry_failed");
            if error.retryable_io() {
                self.defer_storage_retry(now);
            } else {
                self.require_storage_repair("save_rejected");
            }
            return;
        }
        self.durable_state = self.state.clone();
        self.storage_pause = None;
        self.error = None;
        self.revision += 1;
        tracing::info!(
            event = "bus.storage.recovered",
            "Bus state storage recovered"
        );
        let _ = events.send(BusEvent::StorageRecovered);
    }
}

pub(super) fn remove_spooled(
    dir: &std::path::Path,
    paths: &[std::path::PathBuf],
) -> Result<(), String> {
    for path in paths {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())
}
