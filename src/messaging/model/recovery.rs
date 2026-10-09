//! Explicit recovery and terminal failure of unsubmitted queued requests.
use super::{BusState, ModelError, RequestId, RequestPhase, RuntimeStatus};

impl BusState {
    /// Capture all reasons before recovery clears the agent's last refusal.
    pub(crate) fn expire_stalled_queued_requests(
        &mut self,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        let stalled: Vec<_> = self
            .requests
            .values()
            .filter(|request| request.phase == RequestPhase::Queued)
            .filter_map(|request| {
                self.stall_reason(request, now_ms)
                    .map(|reason| (request.id, reason))
            })
            .collect();
        let mut expired = Vec::new();
        for (id, reason) in stalled {
            self.recover_idle_request(id, now_ms)?;
            let request = self
                .requests
                .get_mut(&id)
                .ok_or(ModelError::UnknownRequest(id))?;
            let agent_id = request.agent_id;
            request.failure_reason = Some(reason.clone());
            if let Some(agent) = self
                .agents
                .get_mut(&agent_id)
                .filter(|agent| agent.current_request.is_none())
            {
                agent.actionable_error = Some(format!(
                    "Queued request {} was not submitted ({reason}); it was closed and will not run later. Resend the message when ready.", id.0
                ));
            }
            expired.push(id);
        }
        Ok(expired)
    }

    pub(crate) fn recover_idle_request(
        &mut self,
        request: RequestId,
        recovered_at_ms: u64,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let agent_id = request_state.agent_id;
        if request_state.phase == RequestPhase::Abandoned {
            return Ok(());
        }
        if request_state.phase == RequestPhase::Queued {
            let agent = self
                .agents
                .get(&agent_id)
                .ok_or(ModelError::UnknownAgent(agent_id))?;
            let clear_error = agent.current_request.is_none();
            let queue = self
                .queues
                .get_mut(&agent_id)
                .ok_or(ModelError::InvalidTransition)?;
            let position = queue
                .iter()
                .position(|id| *id == request)
                .ok_or(ModelError::InvalidTransition)?;
            queue.remove(position);
            let request_state = self
                .requests
                .get_mut(&request)
                .ok_or(ModelError::UnknownRequest(request))?;
            request_state.phase = RequestPhase::Abandoned;
            request_state.pending_final = None;
            request_state.completed_at_ms = Some(recovered_at_ms);
            if position == 0 && clear_error {
                if let Some(agent) = self.agents.get_mut(&agent_id) {
                    agent.delivery_rejection = None;
                    agent.actionable_error = None;
                }
            }
            return Ok(());
        }
        if !matches!(
            request_state.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) {
            return Err(ModelError::InvalidTransition);
        }
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if agent.current_request != Some(request) {
            return Err(ModelError::InvalidTransition);
        }
        if agent.status != RuntimeStatus::Idle {
            return Err(ModelError::AgentNotIdle);
        }
        self.abandon_current_request(request, recovered_at_ms)
    }

    /// Abandons `request`, its agent's current one, with its group, and frees the agent.
    pub(super) fn abandon_current_request(
        &mut self,
        request: RequestId,
        recovered_at_ms: u64,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let agent_id = request_state.agent_id;
        request_state.phase = RequestPhase::Abandoned;
        request_state.pending_final = None;
        request_state.completed_at_ms = Some(recovered_at_ms);
        for member in self.group_members(request) {
            if let Some(member) = self.requests.get_mut(&member).filter(|m| !m.settled()) {
                member.phase = RequestPhase::Abandoned;
                member.completed_at_ms = Some(recovered_at_ms);
            }
        }
        let agent = self
            .agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        agent.current_request = None;
        agent.actionable_error = None;
        Ok(())
    }
}
