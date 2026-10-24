//! Explicit recovery and terminal failure of unsubmitted queued requests.
use super::{
    Author, BusState, DialogNoticeDelivery, DialogNoticeTo, ModelError, RequestId, RequestPhase,
    RuntimeStatus, SubmissionOutcome, QUEUED_STALL_MS,
};

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

    /// Only a live dialog episode's Bus-authored notice to its bound
    /// orchestrator can bypass ordinary delivery. Text never grants priority.
    pub(crate) fn is_dialog_notice(&self, id: RequestId) -> bool {
        self.request(id).is_some_and(|request| {
            !request.settled()
                && self.dialog_notice_worker(id).is_some_and(|worker| {
                    self.notice_orchestrator(worker.room_id, Some(worker.id))
                        == Some(request.agent_id)
                })
        })
    }

    /// The persisted episode is independent of its notice request's lifetime.
    fn dialog_notice_worker(&self, id: RequestId) -> Option<&super::RoomAgent> {
        let request = self.request(id)?;
        if request.prompt.author != Author::Bus {
            return None;
        }
        self.agents.values().find(|worker| {
            worker.dialog_notice.is_some()
                && worker.dialog_notice_to == Some(DialogNoticeTo::Orchestrator(id))
                && worker.room_id == request.room_id
        })
    }

    /// Persist alongside begin_submission/begin_steering, before native I/O.
    pub(crate) fn begin_dialog_notice_write(&mut self, id: RequestId, now_ms: u64) {
        if self.dialog_notice_worker(id).is_some() {
            if let Some(request) = self.requests.get_mut(&id) {
                request.dialog_delivery =
                    Some(DialogNoticeDelivery::Unconfirmed { since_ms: now_ms });
            }
        }
    }

    pub(super) fn record_dialog_notice_write(
        &mut self,
        id: RequestId,
        outcome: &SubmissionOutcome,
    ) {
        if self.dialog_notice_worker(id).is_some() {
            if let Some(request) = self.requests.get_mut(&id) {
                match outcome {
                    SubmissionOutcome::Confirmed { .. } => {
                        request.dialog_delivery = Some(DialogNoticeDelivery::Confirmed);
                    }
                    SubmissionOutcome::DefinitelyRejected { .. } => request.dialog_delivery = None,
                    SubmissionOutcome::Uncertain { .. } => {}
                }
            }
        }
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
                    .or_else(|| {
                        (self.is_dialog_notice(request.id)
                            && now_ms.saturating_sub(request.prompt.submitted_at_ms)
                                >= QUEUED_STALL_MS)
                            .then(|| "dialog_notice_undelivered".into())
                    })
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

    /// Run only after fresh dialog observations and durable callbacks. Request
    /// closure, including group settlement and interrupt, is not a receipt.
    pub(crate) fn escalate_unconfirmed_dialog_notices(
        &mut self,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let undelivered: Vec<_> = self
            .agents
            .values()
            .filter_map(|worker| {
                let Some(DialogNoticeTo::Orchestrator(id)) = worker.dialog_notice_to else {
                    return None;
                };
                // A settled request may already live in permanent history.
                let request = self.request(id)?;
                let since_ms = match request.dialog_delivery {
                    Some(DialogNoticeDelivery::Confirmed) => return None,
                    Some(DialogNoticeDelivery::Unconfirmed { since_ms }) => since_ms,
                    // Older saved requests have no explicit receipt/clock.
                    None => request.prompt.submitted_at_ms,
                };
                (!request.trusted_start_bound
                    && self.dialog_notice_worker(request.id).is_some()
                    && now_ms.saturating_sub(since_ms) >= QUEUED_STALL_MS)
                    .then_some(id)
            })
            .collect();
        // Escalation does not replay an unknown write or change its group.
        for id in undelivered {
            self.redirect_dialog_notice(id, "dialog_notice_submission_unconfirmed", now_ms)?;
        }
        Ok(())
    }

    /// An undelivered or unconfirmed blocked notice must reach the developer;
    /// an unknown native write may already have reached the orchestrator.
    fn redirect_dialog_notice(
        &mut self,
        request: RequestId,
        reason: &str,
        now_ms: u64,
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
            "Blocked-dialog notice lacks confirmed delivery; inspect request and dialog state"
        );
        let notice = self
            .request(request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let delivery = if notice.dialog_delivery.is_some()
            || notice.uncertain_outcome
            || notice.steered
            || notice.submission_boundary.is_some()
        {
            "Delivery to the orchestrator is unconfirmed; it may already have received this"
        } else {
            "The orchestrator did not receive this"
        };
        let reason = notice.failure_reason.as_deref().unwrap_or(reason);
        let text = format!("{}\n{delivery} ({reason}).", notice.prompt.text);
        self.post_bus_notice(room, None, text, now_ms)?;
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
