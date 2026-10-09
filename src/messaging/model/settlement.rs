//! Close finished turns after giving their completion callbacks time to arrive.
use super::{AgentId, BusState, RequestId, RuntimeStatus, TURN_SETTLE_MS};

impl BusState {
    pub(super) fn note_turn_end(&mut self, agent: AgentId, now_ms: u64) {
        let Some(agent_state) = self.agents.get(&agent).filter(|agent| !agent.dialog) else {
            return;
        };
        let Some(id) = agent_state.current_request else {
            return;
        };
        let ended = self.requests.get(&id).is_some_and(|request| {
            !request.awaiting_background
                && request.pending_final.is_none()
                && self.turn_ended(request)
        });
        if ended {
            if let Some(request) = self.requests.get_mut(&id) {
                request.turn_ended_at_ms.get_or_insert(now_ms);
            }
        }
    }

    /// Run after callback consumption, so a final spooled at the deadline wins.
    /// No reply is invented when capture failed; the sender sees `abandoned`.
    pub(crate) fn settle_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Option<RequestId> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.deletion_pending
            || agent_state.session_binding_invalidated
            || agent_state.dialog
            || agent_state.status != RuntimeStatus::Idle
        {
            return None;
        }
        let id = agent_state.current_request?;
        let request = self.requests.get(&id)?;
        if request.awaiting_background
            || request.pending_final.is_some()
            || now_ms < request.turn_ended_at_ms?.saturating_add(TURN_SETTLE_MS)
        {
            return None;
        }
        self.abandon_current_request(id, now_ms).ok()?;
        if let Some(agent) = self.agents.get_mut(&agent) {
            agent.actionable_error = Some(
                "Provider turn ended without a captured reply; request closed. Inspect its terminal for the result.".into(),
            );
        }
        tracing::info!(
            event = "bus.message.recovered",
            request_id = id.0,
            agent_id = agent.0,
            reason = "reply_not_captured",
            "Finished turn released"
        );
        Some(id)
    }
}
