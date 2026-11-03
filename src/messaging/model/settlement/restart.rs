//! Reconcile requests interrupted by a newer native-server generation.
use super::{AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase};

impl BusState {
    /// Reconcile one older-generation request. Newer group members keep their
    /// own lifecycle; callbacks and provider records settle the actual final
    /// before an unresolved old group closes.
    pub(crate) fn settle_generation_request(
        &mut self,
        agent: AgentId,
        request: RequestId,
        generation: u64,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        if owner.current_request != Some(request)
            || owner
                .provider_retry
                .as_ref()
                .is_some_and(|hold| hold.lead == request)
        {
            return None;
        }
        let request = self.requests.get(&request)?.clone();
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) || request.pending_final.is_some()
        {
            return None;
        }
        let id = request.id;
        if generation > 0 {
            for member in self.group_members(id) {
                if self.requests.get(&member).is_some_and(|member| {
                    member
                        .delivery_epoch
                        .as_ref()
                        .is_some_and(|epoch| epoch.generation >= generation)
                }) {
                    self.requests.get_mut(&member).unwrap().group = None;
                }
            }
        }
        self.consume_recorded_end(
            agent,
            id,
            now_ms,
            recorded_end(&request),
            "restart",
            false,
            "Bus restarted before the request completed; inspect its terminal.",
            "Bus restarted before a reply was captured; the in-flight turn was interrupted. Inspect its terminal and resend if needed.",
        )
    }
}
