//! Close queued stalls before delivery/status, and unconfirmed writes only after reconciliation.
use super::{diagnostics, io, Worker};

/// Extra time for a due notice's native dialog read to recover. Anchored to
/// the persisted delivery deadline, so repeated reads and restarts cannot renew it.
pub(super) const DIALOG_OBSERVATION_GRACE_MS: u64 = 3 * 60 * 1000;

impl Worker {
    pub(super) fn expire_queued_requests(&mut self, now_ms: u64) -> Result<(), String> {
        self.pause_stalled_reply_deliveries(now_ms);
        let mut state = self.state.clone();
        let expired = state
            .expire_stalled_queued_requests(now_ms)
            .map_err(|e| e.to_string())?;
        state.retain_worker_outcomes(now_ms);
        if state != self.state {
            self.save(state)?;
        }
        for id in expired {
            diagnostics::request(&self.state, id, "bus.message.failed", "queued_stalled");
        }
        Ok(())
    }

    pub(super) fn expire_queued_now(&mut self) -> Result<(), String> {
        self.expire_queued_requests(io::now_ms())
    }

    /// Call only after a fresh poll and durable callback reconciliation, so
    /// already-spooled replies and current Working status win over expiry.
    pub(super) fn expire_unconfirmed_submissions(&mut self, now_ms: u64) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .fail_unconfirmed_submissions(now_ms)
            .map_err(|e| e.to_string())?;
        for (worker, notice, delivery_deadline) in state.dialog_notice_fallback_candidates(now_ms) {
            let observation =
                self.dialog_identity(worker)
                    .and_then(|(_, _, terminal, pane, session)| {
                        self.native_dialog(&pane, &terminal, session.as_deref(), false)
                    });
            // The notification marker is debounced history, not proof of a
            // still-open screen. A readable close always wins; an unreadable
            // snapshot can defer attention only until its fixed second bound.
            let reason = match observation {
                Ok(current) if current.dialog.is_some() || current.pending_question => {
                    Some("dialog_notice_submission_unconfirmed")
                }
                Err(_)
                    if now_ms >= delivery_deadline.saturating_add(DIALOG_OBSERVATION_GRACE_MS) =>
                {
                    Some("dialog_notice_observation_unavailable")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                state
                    .redirect_dialog_notice(notice, reason, now_ms)
                    .map_err(|e| e.to_string())?;
            }
        }
        state.retain_worker_outcomes(now_ms);
        if state != self.state {
            self.save(state)?;
        }
        Ok(())
    }
}
