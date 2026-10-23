//! Explicit recovery and terminal failure of unsubmitted queued requests.
use super::{BusState, DialogNoticeTo, ModelError, RequestId, RequestPhase, RuntimeStatus};

impl BusState {
    /// Close only never-submitted notices emitted by the retired GitHub watch.
    /// Ordinary agent/developer messages and already typed work are retained.
    pub(crate) fn retire_queued_github_notices(&mut self, now_ms: u64) -> Result<(), ModelError> {
        let retired: Vec<_> = self
            .requests
            .values()
            .filter(|request| {
                request.phase == RequestPhase::Queued
                    && request.prompt.author == super::Author::Bus
                    && request.prompt.text.starts_with("GitHub: ")
            })
            .map(|request| request.id)
            .collect();
        for id in retired {
            self.recover_idle_request(id, now_ms)?;
            self.requests
                .get_mut(&id)
                .ok_or(ModelError::UnknownRequest(id))?
                .failure_reason = Some("github_watch_retired".into());
        }
        Ok(())
    }

    /// Retire only never-submitted digests from older saved sessions. Typed
    /// requests keep their original ownership and history.
    pub(crate) fn retire_queued_room_digests(&mut self, now_ms: u64) -> Result<bool, ModelError> {
        let retired: Vec<_> = self
            .requests
            .values()
            .filter(|request| {
                request.phase == RequestPhase::Queued
                    && request.prompt.author == super::Author::Bus
                    && request.prompt.text.starts_with("Room digest:")
            })
            .map(|request| request.id)
            .collect();
        for id in &retired {
            self.recover_idle_request(*id, now_ms)?;
            self.requests
                .get_mut(id)
                .ok_or(ModelError::UnknownRequest(*id))?
                .failure_reason = Some("room_digest_retired".into());
        }
        Ok(!retired.is_empty())
    }

    /// Retire only never-submitted operational inputs from older versions.
    pub(crate) fn retire_queued_operational_notices(
        &mut self,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let retired: Vec<_> = self
            .requests
            .values()
            .filter(|request| {
                request.phase == RequestPhase::Queued && request.prompt.author == super::Author::Bus
            })
            .filter_map(|request| {
                let text = request.prompt.text.as_str();
                let reason = if text.starts_with("Bus messaging rules updated.") {
                    "messaging_onboarding_retired"
                } else if (text.starts_with("BLOCKED ") && text.contains(". Startup failed:"))
                    || text.starts_with("Deleted agent \"")
                {
                    "operational_notice_retired"
                } else {
                    return None;
                };
                Some((request.id, reason))
            })
            .collect();
        for (id, reason) in retired {
            self.recover_idle_request(id, now_ms)?;
            self.requests
                .get_mut(&id)
                .ok_or(ModelError::UnknownRequest(id))?
                .failure_reason = Some(reason.into());
        }
        Ok(())
    }

    /// Capture all reasons before recovery clears the agent's last refusal.
    pub(crate) fn expire_stalled_queued_requests(
        &mut self,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.record_stalled_errors(now_ms)?;
        let stalled: Vec<_> = self
            .requests
            .values()
            .filter(|request| request.phase == RequestPhase::Queued)
            .filter(|request| !self.is_reply_notification(request.id))
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
            self.redirect_dialog_notice(id, &reason, now_ms)?;
            expired.push(id);
        }
        Ok(expired)
    }

    /// A blocked notice the orchestrator never took would leave the dialog
    /// unreported for as long as it stays open, so the developer gets it.
    fn redirect_dialog_notice(
        &mut self,
        request: RequestId,
        reason: &str,
        _now_ms: u64,
    ) -> Result<(), ModelError> {
        let Some((agent, room)) = self
            .agents
            .values()
            .find(|agent| agent.dialog_notice_to == Some(DialogNoticeTo::Orchestrator(request)))
            .map(|agent| (agent.id, agent.room_id))
        else {
            return Ok(());
        };
        tracing::warn!(
            event = "bus.dialog.notice_undelivered",
            request_id = request.0,
            agent_id = agent.0,
            room_id = room.0,
            reason,
            "Blocked-dialog notice was not delivered; inspect request and dialog state"
        );
        self.set_dialog_notice_to(agent, Some(DialogNoticeTo::Developer))
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
        // Explicitly close ownership when the invalidated session cannot resume.
        // Mere unavailability can be transient and must not abandon typed work.
        let invalidated =
            agent.status == RuntimeStatus::Unavailable && agent.session_binding_invalidated;
        if agent.status != RuntimeStatus::Idle && !invalidated {
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
