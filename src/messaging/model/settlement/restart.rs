//! Reconcile tasks interrupted by a server restart or proven runtime loss.
use super::super::{restart_continuation::ContinuationPreparation, RestartResume};
use super::{AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase};

impl BusState {
    /// Recover old execution evidence before continuing its unresolved inputs.
    /// Current-generation inputs remain independent. A failed cold resume
    /// cannot keep its interrupted provider retry hold.
    pub(crate) fn settle_generation_request(
        &mut self,
        agent: AgentId,
        id: RequestId,
        generation: u64,
        now_ms: u64,
        mut recorded_end: impl FnMut(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        let request = self.requests.get(&id)?.clone();
        let resume_failed = request.restart_resume == Some(RestartResume::Failed)
            || owner.session_binding_invalidated
            || owner.working_directory_error.is_some()
            || owner.missing_binary.is_some()
            || owner
                .startup
                .as_ref()
                .is_some_and(|startup| startup.failure.is_some());
        if request.agent_id != agent
            || owner.deletion_pending
            || request.restart_resume.is_none()
                && request
                    .delivery_epoch
                    .as_ref()
                    .is_none_or(|epoch| epoch.generation >= generation)
            || request.group.is_some_and(|lead| {
                owner.current_request == Some(lead)
                    && self
                        .requests
                        .get(&lead)
                        .is_some_and(|r| r.restart_resume.is_some())
            })
            || !(matches!(
                request.phase,
                RequestPhase::Submitting | RequestPhase::Active
            ) || request.restart_continuation && request.phase == RequestPhase::Queued
                || request.phase == RequestPhase::Completed && request.pending_final.is_some())
            || !resume_failed
                && owner
                    .provider_retry
                    .as_ref()
                    .is_some_and(|hold| hold.lead == id || request.group == Some(hold.lead))
        {
            return None;
        }
        let current = owner.current_request;
        // Closing the turn calls abandon_current_request, which clears the
        // agent's hold even when this request does not own it.
        // finish_completed_turn clears a hold only for its own lead; keep a
        // hold that still belongs to a different delivery.
        let retained_hold = owner.provider_retry.clone();
        let mut successor = None;
        for member in self.group_members(id) {
            let member = self.requests.get_mut(&member)?;
            if !member.settled()
                && request
                    .delivery_epoch
                    .as_ref()
                    .is_some_and(|epoch| epoch.generation < generation)
                && member
                    .delivery_epoch
                    .as_ref()
                    .is_some_and(|epoch| epoch.generation >= generation)
            {
                let lead = *successor.get_or_insert(member.id);
                member.group = (member.id != lead).then_some(lead);
            }
        }
        if current != Some(id) {
            self.requests.get_mut(&id)?.group = None;
        }
        self.agents.get_mut(&agent)?.current_request = Some(id);
        // An answered lead's recorded turn can still claim unresolved steering.
        if let Some(RecordedTurnEnd::Final {
            turn_id, prompts, ..
        }) = recorded_end(&request)
        {
            self.bind_recorded_turn_inputs(agent, &turn_id, &prompts);
        }
        self.complete_pending_final(agent, now_ms).ok()?;
        let members = self.group_members(id);
        for member in &members {
            if let Some(member) = self
                .requests
                .get(member)
                .filter(|member| !member.settled())
                .cloned()
            {
                self.recover_steered_answer(
                    &request,
                    &member,
                    &members,
                    recorded_end(&member),
                    now_ms,
                );
            }
        }
        if (request.trusted_start_bound || request.restart_continuation)
            && request.pending_final.is_none()
            && recorded_end(&request).is_none()
            && !resume_failed
        {
            let preparation = self.prepare_restart_continuation(agent, id, generation, now_ms);
            if preparation != ContinuationPreparation::Refused {
                if preparation == ContinuationPreparation::AwaitingResume {
                    self.requests
                        .get_mut(&id)?
                        .restart_resume
                        .get_or_insert(RestartResume::Awaiting);
                }
                self.restore_interrupted_owner(agent, id, current, successor)?;
                return Some(id);
            }
        }
        self.settle_interrupted_generation_end(
            agent,
            id,
            now_ms,
            recorded_end(&request),
            resume_failed,
        )?;
        if let Some(mut answered) = self
            .requests
            .get(&id)
            .filter(|request| request.pending_final.is_some())
            .cloned()
        {
            self.close_turn_with_reason(id, now_ms, "reply_not_captured".into())
                .ok()?;
            answered.pending_final = None;
            self.requests.insert(id, answered);
        }
        let restore_hold = retained_hold
            .filter(|hold| hold.lead != id && !self.group_members(hold.lead).contains(&id));
        let owner = self.agents.get_mut(&agent)?;
        if owner.provider_retry.is_none() {
            owner.provider_retry = restore_hold;
        }
        owner.current_request = current
            .filter(|previous| *previous != id)
            .or(successor)
            .or(owner.current_request);
        Some(id)
    }

    /// Reconciliation of older work cannot release a newer delivery's owner.
    fn restore_interrupted_owner(
        &mut self,
        agent: AgentId,
        id: RequestId,
        current: Option<RequestId>,
        successor: Option<RequestId>,
    ) -> Option<()> {
        if let Some(current) = current.filter(|current| *current != id).or(successor) {
            self.agents.get_mut(&agent)?.current_request = Some(current);
            if self.requests.get(&current).is_some_and(|request| {
                request.restart_continuation && request.phase == RequestPhase::Queued
            }) {
                let queue = self.queues.entry(agent).or_default();
                queue.retain(|request| *request != current);
                queue.insert(0, current);
            }
        }
        Some(())
    }

    fn settle_interrupted_generation_end(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded: Option<RecordedTurnEnd>,
        resume_failed: bool,
    ) -> Option<()> {
        let unresolved = !self.requests.get(&id)?.settled()
            || self.group_members(id).iter().any(|member| {
                self.requests
                    .get(member)
                    .is_some_and(|request| !request.settled())
            });
        if unresolved {
            // A failed cold resume cannot retain a retry hold for a dead turn.
            // Committed finals still settle before failure fallback.
            let recorded = if resume_failed && matches!(&recorded, Some(RecordedTurnEnd::Error(_)))
            {
                None
            } else {
                recorded
            };
            self.consume_recorded_end(
                agent, id, now_ms, recorded, "restart", false,
                "Bus restarted before the request completed; inspect its terminal.",
                Some("Bus restarted before a reply was captured; the in-flight turn was interrupted. Inspect its terminal and resend if needed."),
            )?;
        }
        Some(())
    }
}
