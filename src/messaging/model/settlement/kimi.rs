//! Settle Kimi only from its owned native terminal commit.
use super::{
    AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase, RuntimeStatus,
    TURN_SETTLE_MS,
};

impl BusState {
    /// An owned Kimi wire commit ends the turn even while its screen still
    /// looks busy or blocked. Process exit without a commit keeps its grace.
    pub(super) fn settle_kimi_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        if owner.deletion_pending || owner.session_binding_invalidated {
            return None;
        }
        let request = self.requests.get(&owner.current_request?)?;
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) || request.awaiting_background
            || request.pending_final.is_some()
        {
            return None;
        }
        let id = request.id;
        let exited = owner.status == RuntimeStatus::Unavailable
            && owner.exit_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.launch_id == request.expected_launch_id
                    && now_ms >= snapshot.at_ms.saturating_add(TURN_SETTLE_MS)
            });
        if !(request.trusted_start_bound || exited && request.phase == RequestPhase::Active) {
            return None;
        }
        let recorded = recorded_end(request);
        let held = owner
            .provider_retry
            .as_ref()
            .is_some_and(|hold| owner.current_request == Some(hold.lead));
        if held
            && !matches!(&recorded, Some(RecordedTurnEnd::Error(error))
                if crate::agents::providers::kimi::quota_exhausted(error))
        {
            return None;
        }
        if recorded.is_none() && self.idle_bound_turn_candidate(agent, now_ms) == Some(id) {
            return self.settle_idle_bound_turn(agent, id, now_ms, None);
        }
        if recorded.is_none() && (request.restart_resume.is_some() || owner.dialog || !exited) {
            return None;
        }
        self.consume_recorded_end(
            agent,
            id,
            now_ms,
            recorded,
            "kimi-wire",
            exited,
            "Kimi process exited after its turn ended; inspect its terminal.",
            None,
        )
    }
}
