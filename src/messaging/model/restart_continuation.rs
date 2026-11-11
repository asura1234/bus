//! Continue interrupted tasks in their saved conversation, retaining request IDs.
use super::{
    callbacks::provider_turn_key, AgentId, Author, BusState, DeliveryEpoch, RequestId, RequestPhase,
};

impl BusState {
    pub(super) fn restart_continuation_failure_reason(
        &self,
        id: RequestId,
        reason: String,
    ) -> String {
        if reason == "submission_unconfirmed"
            && self
                .requests
                .get(&id)
                .is_some_and(|r| r.restart_continuation && !r.trusted_start_bound)
        {
            "reply_not_captured".to_owned()
        } else {
            reason
        }
    }

    pub(super) fn restart_continuation_notice(&self, id: RequestId) -> Option<String> {
        let request = self.requests.get(&id)?;
        let sender = match &request.prompt.author {
            Author::Developer => "you".to_owned(),
            Author::Bus => "bus".to_owned(),
            Author::Agent(agent) => request
                .sender_identity
                .as_ref()
                .map(|(name, _)| name.clone())
                .unwrap_or_else(|| self.agent_message_label(*agent)),
        };
        let message = request.prompt.id.0;
        let joined = self
            .group_members(id)
            .into_iter()
            .filter_map(|id| self.requests.get(&id).filter(|member| !member.settled()))
            .map(|member| member.prompt.id.0.to_string())
            .collect::<Vec<_>>();
        let joined = if joined.is_empty() {
            String::new()
        } else {
            format!(
                " Messages {} are still part of this task.",
                joined.join(", ")
            )
        };
        Some(format!(
            "[Bus message {message} from {sender}]\nBus restarted while you were working on message {message} from {sender}; your previous turn was interrupted. Check what you already did, then continue and finish that task.{joined} Reply as usual to the original message IDs."
        ))
    }

    /// Requeue only a notice for the interrupted execution. Keep its group and
    /// current owner until that same lead is submitted in the new generation.
    pub(super) fn prepare_restart_continuation(
        &mut self,
        agent: AgentId,
        id: RequestId,
        generation: u64,
        now_ms: u64,
    ) -> bool {
        let Some(owner) = self.agents.get(&agent) else {
            return false;
        };
        if owner.deletion_pending
            || owner.session_binding_invalidated
            || owner.working_directory_error.is_some()
            || owner.missing_binary.is_some()
            || owner.startup.as_ref().is_some_and(|s| s.failure.is_some())
        {
            return false;
        }
        let identity = owner.runtime_identity.clone();
        let (Some(terminal), Some(pane)) = (&identity.terminal_id, &identity.pane_id) else {
            return false;
        };
        let all: Vec<_> = std::iter::once(id).chain(self.group_members(id)).collect();
        let pending: Vec<_> = all
            .iter()
            .copied()
            .filter(|id| self.requests.get(id).is_some_and(|r| !r.settled()))
            .collect();
        // No callback from the killed execution may bind or finish its successor.
        for id in &all {
            let request = &self.requests[id];
            if let Some(key) = provider_turn_key(
                request.expected_launch_id.as_deref().unwrap_or_default(),
                request.provider_session_id.as_deref(),
                request.provider_turn_id.as_deref(),
            ) {
                self.retried_provider_turns.insert(key.clone());
                self.consumed_provider_turns.insert(key);
            }
        }
        for id in all.iter().filter(|id| !pending.contains(id)) {
            if let Some(request) = self.requests.get_mut(id) {
                request.phase = RequestPhase::Completed;
                request.pending_final = None;
                request.group = None;
            }
        }
        let Some(lead) = pending.first().copied() else {
            self.agents
                .get_mut(&agent)
                .expect("retained agent")
                .current_request = None;
            return true;
        };
        for id in &pending {
            let request = self.requests.get_mut(id).expect("retained request");
            request.phase = if *id == lead {
                RequestPhase::Queued
            } else {
                RequestPhase::Active
            };
            request.group = (*id != lead).then_some(lead);
            request.restart_continuation = true;
            request.steered = false;
            request.delivery_epoch = Some(DeliveryEpoch {
                generation,
                terminal_id: terminal.clone(),
                pane_id: pane.clone(),
            });
            request.expected_launch_id = identity.launch_id.clone();
            request.provider_session_id = identity.session_id.clone();
            request.provider_turn_id = None;
            request.provider_prompt_id = None;
            request.submission_boundary = None;
            request.submission_status_revision = 0;
            request.trusted_start_bound = false;
            request.uncertain_outcome = false;
            request.failure_reason = None;
            request.submission_error = None;
            request.queued_rejection = None;
            request.pending_final = None;
            request.completed_at_ms = None;
            request.submitted_payload = None;
            request.foreign_turn_settled_at_ms = None;
            request.progress_at_ms = Some(now_ms);
            request.turn_ended_at_ms = None;
            request.awaiting_background = false;
            request.latency.clear_attempt();
        }
        let notice = self
            .restart_continuation_notice(lead)
            .expect("retained lead");
        for id in &pending {
            self.requests
                .get_mut(id)
                .expect("retained request")
                .submitted_payload = Some(notice.clone());
        }
        let queue = self.queues.entry(agent).or_default();
        queue.retain(|id| !pending.contains(id));
        queue.insert(0, lead);
        let owner = self.agents.get_mut(&agent).expect("retained agent");
        owner.current_request = Some(lead);
        owner.delivery_rejection = None;
        true
    }

    /// All preserved inputs belong to the resumed turn, without pasting them again.
    pub(super) fn bind_restart_continuation_group(&mut self, lead: RequestId) {
        let Some(anchor) = self.requests.get(&lead).cloned() else {
            return;
        };
        for id in self.group_members(lead) {
            if let Some(member) = self
                .requests
                .get_mut(&id)
                .filter(|r| !r.settled() && r.restart_continuation)
            {
                member.phase = RequestPhase::Active;
                member.expected_launch_id = anchor.expected_launch_id.clone();
                member.submission_boundary = anchor.submission_boundary;
                member.submission_status_revision = anchor.submission_status_revision;
                member.provider_session_id = anchor.provider_session_id.clone();
                member.provider_turn_id = anchor.provider_turn_id.clone();
                member.provider_prompt_id = anchor.provider_prompt_id.clone();
                member.trusted_start_bound = true;
            }
        }
    }
}
