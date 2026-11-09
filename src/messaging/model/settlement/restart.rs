//! Reconcile requests interrupted by a newer native-server generation.
use super::{AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase};

impl BusState {
    /// Recover old execution evidence before closing its unresolved inputs.
    /// Current-generation inputs and provider retry ownership remain independent.
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
        if request.agent_id != agent
            || owner.deletion_pending
            || owner.session_binding_invalidated
            || request
                .delivery_epoch
                .as_ref()
                .is_none_or(|epoch| epoch.generation >= generation)
            || !(matches!(
                request.phase,
                RequestPhase::Submitting | RequestPhase::Active
            ) || request.phase == RequestPhase::Completed && request.pending_final.is_some())
            || owner
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
        if !self.requests.get(&id)?.settled() {
            self.consume_recorded_end(
                agent,
                id,
                now_ms,
                recorded_end(&request),
                "restart",
                false,
                "Bus restarted before the request completed; inspect its terminal.",
                Some("Bus restarted before a reply was captured; the in-flight turn was interrupted. Inspect its terminal and resend if needed."),
            )?;
        }
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
}
