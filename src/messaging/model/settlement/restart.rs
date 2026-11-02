//! Reconcile requests interrupted by native resume or a recorded restart.
use super::{AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase};

impl BusState {
    /// Reconcile a request that was current when this coordinator opened. The
    /// stop interrupted its old turn, so a later Working-to-Idle transition or
    /// process-exit snapshot cannot be required. Callbacks and provider records
    /// still settle the actual final before an unresolved group closes.
    pub(crate) fn settle_restarted_request(
        &mut self,
        agent: AgentId,
        request: RequestId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        if self.agents.get(&agent)?.current_request != Some(request) {
            return None;
        }
        let request = self.requests.get(&request)?;
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) || request.pending_final.is_some()
        {
            return None;
        }
        let id = request.id;
        self.consume_recorded_end(
            agent,
            id,
            now_ms,
            recorded_end(request),
            "restart",
            false,
            "Bus restarted before the request completed; inspect its terminal.",
            "Bus restarted before a reply was captured; the in-flight turn was interrupted. Inspect its terminal and resend if needed.",
        )
    }
}
