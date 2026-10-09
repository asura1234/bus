//! Close untyped, stalled work before any delivery attempt or status response.
use super::{diagnostics, io, RequestPhase, Worker};

impl Worker {
    pub(super) fn expire_queued_requests(&mut self, now_ms: u64) -> Result<(), String> {
        if !self.state.requests().any(|request| {
            request.phase == RequestPhase::Queued
                && self.state.stall_reason(request, now_ms).is_some()
        }) {
            return Ok(());
        }
        let mut state = self.state.clone();
        let expired = state
            .expire_stalled_queued_requests(now_ms)
            .map_err(|e| e.to_string())?;
        if expired.is_empty() {
            return Ok(());
        }
        self.save(state)?;
        for id in expired {
            diagnostics::request(&self.state, id, "bus.message.failed", "queued_stalled");
        }
        Ok(())
    }

    pub(super) fn expire_queued_now(&mut self) -> Result<(), String> {
        self.expire_queued_requests(io::now_ms())
    }
}
