//! Close untyped, stalled work before any delivery attempt or status response.
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
}
