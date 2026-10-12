//! Close finished turns after giving their completion callbacks time to arrive.
use super::{AgentId, BusState, Request, RequestId, RuntimeStatus, TURN_SETTLE_MS};

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
    /// No reply is invented when capture failed; the sender sees `abandoned`
    /// with a reason: `provider_error: ...` when `provider_error` finds the
    /// error that ended the turn, else `reply_not_captured`.
    pub(crate) fn settle_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        provider_error: impl FnOnce(&Request) -> Option<String>,
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
        let provider_error = provider_error(request);
        // Messages typed or coalesced into this turn close with it and share
        // its reason; members that already settled keep their own outcome.
        let mut closed = self.group_members(id);
        closed.retain(|member| self.requests.get(member).is_some_and(|m| !m.settled()));
        closed.push(id);
        self.abandon_current_request(id, now_ms).ok()?;
        let (reason, error) = match &provider_error {
            Some(message) => (
                format!("provider_error: {message}"),
                format!("Provider turn failed: {message}; request closed. Resend it to retry."),
            ),
            None => (
                "reply_not_captured".to_owned(),
                "Provider turn ended without a captured reply; request closed. Inspect its terminal for the result.".to_owned(),
            ),
        };
        for request in closed {
            if let Some(request) = self.requests.get_mut(&request) {
                request.failure_reason = Some(reason.clone());
            }
        }
        if let Some(agent) = self.agents.get_mut(&agent) {
            agent.actionable_error = Some(error);
        }
        tracing::info!(
            event = "bus.message.recovered",
            request_id = id.0,
            agent_id = agent.0,
            reason = if provider_error.is_some() {
                "provider_error"
            } else {
                "reply_not_captured"
            },
            "Finished turn released"
        );
        Some(id)
    }
}
