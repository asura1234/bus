//! Close queued stalls before delivery/status, and unconfirmed writes only after reconciliation.
use super::{diagnostics, io, Worker};

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
        state
            .escalate_unconfirmed_dialog_notices(now_ms)
            .map_err(|e| e.to_string())?;
        state.retain_worker_outcomes(now_ms);
        if state != self.state {
            self.save(state)?;
        }
        Ok(())
    }
}
