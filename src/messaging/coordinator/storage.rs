//! Durable storage pause and recovery for the single coordinator.
use super::{diagnostics, io, mpsc, BusEvent, BusState, Duration, RequestId, Worker};
use crate::messaging::storage::state_store::{JournalOp, StoreError};
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

    pub(super) fn note_requests_queued(&mut self, ids: &[RequestId]) {
        let at_ms = io::now_ms();
        for id in ids {
            if let Some(agent) = self
                .state
                .request(*id)
                .and_then(|r| self.state.agent(r.agent_id))
                .cloned()
            {
                self.note_delivery_eligibility(&agent);
            }
            let mut state = self.state.clone();
            let Some(room) = state.request(*id).map(|request| request.room_id) else {
                continue;
            };
            if state.record_queued(*id, at_ms).is_err() {
                continue;
            }
            if self
                .journal(
                    state.clone(),
                    JournalOp::DeliveryTimestamps {
                        room,
                        request_id: *id,
                        queued_at_ms: Some(at_ms),
                        eligible_at_ms: state.request(*id).and_then(|r| r.latency.eligible_at_ms),
                        native_requested_at_ms: None,
                        reply_delivered_at_ms: None,
                    },
                )
                .is_err()
            {
                continue;
            }
            let completed_at_ms = io::now_ms();
            let mut completed = state.clone();
            if completed.finish_queued(*id, at_ms, completed_at_ms).is_ok() {
                let _ = self.journal(
                    completed,
                    JournalOp::complete_queued(room, *id, at_ms, completed_at_ms),
                );
            }
        }
    }

    /// Commits `state` and makes it durable. A state that differs from the
    /// last write only in status-only observations writes nothing: those are
    /// never persisted on their own (see `apply_poll`), so rewriting the same
    /// durable content is pure cost. Inside a batch the write waits for
    /// [`Self::finish_batch`].
    pub(super) fn save(&mut self, state: BusState) -> Result<(), String> {
        self.save_with_reply_timestamps(state, true)
    }

    pub(super) fn save_without_reply_timestamps(&mut self, state: BusState) -> Result<(), String> {
        self.save_with_reply_timestamps(state, false)
    }

    fn save_with_reply_timestamps(
        &mut self,
        state: BusState,
        stamp_new_replies: bool,
    ) -> Result<(), String> {
        if !self.store.has_unfolded_journal()
            && !state.durable_poll_change_from(&self.durable_state)
        {
            if state != self.state {
                self.state = state;
                self.revision += 1;
            }
            return Ok(());
        }
        if let Some(batch) = self.batch.as_mut() {
            if batch.before.is_none() {
                batch.before = Some((self.state.clone(), self.revision));
            }
            self.state = state;
            self.revision += 1;
            return Ok(());
        }
        self.write_with_reply_timestamps(state, stamp_new_replies)?;
        self.revision += 1;
        Ok(())
    }

    pub(super) fn write(&mut self, state: BusState) -> Result<(), String> {
        self.write_with_reply_timestamps(state, true)
    }

    fn write_with_reply_timestamps(
        &mut self,
        state: BusState,
        stamp_new_replies: bool,
    ) -> Result<(), String> {
        if let Err(error) = self.store.save(&state) {
            if crate::utils::error_pipe::is_suppressed() {
                tracing::error!(event = "bus.error_pipe.save_failed", %error, "Error notice could not be persisted");
                return Err(error.to_string());
            }
            self.pause_storage(&error);
            return Err(self.storage_notice().into());
        }
        // Reply stamps are journaled after state.json. durable_state stays the
        // full commit so a retry can tell a torn write from that journal,
        // except a reply whose journal failed: that completed_reply stays off
        // durable_state until a later save journals the stamp.
        let mut live = state.clone();
        if stamp_new_replies {
            let at_ms = io::now_ms();
            let new_replies: Vec<RequestId> = state
                .requests()
                .filter(|request| {
                    request.completed_reply.is_some()
                        && self
                            .durable_state
                            .request(request.id)
                            .and_then(|old| old.completed_reply.as_ref())
                            .is_none()
                })
                .map(|request| request.id)
                .collect();
            for id in new_replies {
                let mut delivered = live.clone();
                delivered.record_reply_delivered(id, at_ms);
                let Some(room) = delivered.request(id).map(|request| request.room_id) else {
                    continue;
                };
                if self
                    .journal(
                        delivered.clone(),
                        JournalOp::DeliveryTimestamps {
                            room,
                            request_id: id,
                            queued_at_ms: None,
                            eligible_at_ms: None,
                            native_requested_at_ms: None,
                            reply_delivered_at_ms: Some(at_ms),
                        },
                    )
                    .is_ok()
                {
                    live = delivered;
                    self.unstamped_replies.remove(&id);
                } else {
                    // The state.json commit already holds completed_reply.
                    // Leave it off durable_state so the next save journals
                    // the stamp. Disk still matches except for this field.
                    self.unstamped_replies.insert(id);
                }
            }
        }
        diagnostics::replies(&self.durable_state, &state, "bus.reply.persisted");
        diagnostics::recoveries(&self.durable_state, &state);
        diagnostics::provider_resets(&self.durable_state, &state);
        super::launch_metrics::log_outcomes(&self.durable_state, &state);
        for agent in state.agents() {
            if self
                .durable_state
                .agent(agent.id)
                .is_some_and(|old| old.status != agent.status)
            {
                tracing::debug!(event = "bus.agent.status", agent_id = agent.id.0,
                    room_id = agent.room_id.0, request_id = ?agent.current_request.map(|id| id.0),
                    status = ?agent.status, "Bus status changed");
            }
        }
        self.durable_state = state;
        self.durable_state
            .hide_completed_replies(&self.unstamped_replies);
        self.state = live;
        Ok(())
    }

    /// Disk may still hold a reply whose delivery journal failed. That gap
    /// is `unstamped_replies`, not a torn state.json.
    fn committed_state_matches(&self, mut committed: BusState) -> bool {
        committed.hide_completed_replies(&self.unstamped_replies);
        committed == self.durable_state
    }

    /// Commits a composer draft edit or send through the store's journal:
    /// durable before it is acknowledged, without a full `state.json` save.
    pub(super) fn journal(&mut self, state: BusState, op: JournalOp) -> Result<(), String> {
        if let Err(error) = self.store.journal(op) {
            self.pause_storage(&error);
            return Err(self.storage_notice().into());
        }
        self.state = state;
        self.revision += 1;
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
            // A pattern guard cannot move `durable`, so the hide-and-compare
            // runs in the arm.
            Ok(Some(durable)) => {
                if !self.committed_state_matches(durable) {
                    self.require_storage_repair("divergent_or_missing");
                    return;
                }
            }
            Ok(None) => {
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
        diagnostics::recoveries(&self.durable_state, &self.state);
        super::launch_metrics::log_outcomes(&self.durable_state, &self.state);
        self.durable_state = self.state.clone();
        self.durable_state
            .hide_completed_replies(&self.unstamped_replies);
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
