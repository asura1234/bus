//! Close finished turns after giving their completion callbacks time to arrive.
use super::{
    types::ShellHold, AgentId, BusState, ModelError, PendingFinal, Request, RequestId,
    RequestPhase, RuntimeStatus, HELD_REPLY_SETTLE_MS, TURN_SETTLE_MS,
};

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

    /// Retain the agent's current reply while background work remains.
    /// The cap counts from the first hold, on the status
    /// clock that `release_shell_hold` reads; the newest interim text is what
    /// the cap delivers.
    pub(crate) fn hold_background_reply(&mut self, agent: AgentId, interim: PendingFinal) {
        let Some((id, observed_at_ms)) = self
            .agents
            .get(&agent)
            .and_then(|a| Some((a.current_request?, a.observed_at_ms)))
        else {
            return;
        };
        let Some(request) = self.requests.get_mut(&id).filter(|request| {
            request.awaiting_background
                && request.provider_session_id == interim.provider_session_id
                && request.provider_turn_id == interim.provider_turn_id
        }) else {
            return;
        };
        let since_ms = request
            .shell_hold
            .as_ref()
            .map_or(observed_at_ms, |hold| hold.since_ms);
        request.shell_hold = Some(ShellHold { since_ms, interim });
    }

    /// Past `cap_ms` of holding for its shells, the request takes its newest
    /// interim text as the reply, with reason `background_shells_timeout`, so
    /// background work that never exits cannot hold it or its queue forever.
    /// The historical reason and setting names remain compatible.
    pub(crate) fn release_shell_hold(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        cap_ms: u64,
    ) -> Option<RequestId> {
        let id = self.agents.get(&agent)?.current_request?;
        let request = self.requests.get(&id)?;
        let hold = request.shell_hold.as_ref()?;
        if !request.has_held_background_reply()
            || request.settled()
            || now_ms < hold.since_ms.saturating_add(cap_ms)
        {
            return None;
        }
        let interim = hold.interim.clone();
        self.deliver_held_interim(agent, id, interim, now_ms, "background_shells_timeout")
    }

    /// Settles the agent's current request `id`, with its group, on `interim`:
    /// the newest text a `Stop` gave while the turn held for its shells.
    /// `reason` says why the hold ended short of the real reply.
    fn deliver_held_interim(
        &mut self,
        agent: AgentId,
        id: RequestId,
        interim: PendingFinal,
        now_ms: u64,
        reason: &str,
    ) -> Option<RequestId> {
        let mut closed = self.group_members(id);
        closed.retain(|member| self.requests.get(member).is_some_and(|m| !m.settled()));
        closed.push(id);
        let request = self.requests.get_mut(&id)?;
        request.awaiting_background = false;
        request.turn_ended_at_ms = None;
        request.pending_final = Some(interim);
        if !self.complete_pending_final(agent, now_ms).ok()? {
            return None;
        }
        for request in closed {
            if let Some(request) = self.requests.get_mut(&request) {
                request.failure_reason = Some(reason.to_owned());
            }
        }
        tracing::info!(
            event = "bus.message.recovered",
            request_id = id.0,
            agent_id = agent.0,
            reason,
            "Held reply delivered"
        );
        Some(id)
    }

    /// A typed message that starts a different turn must not replace an
    /// answer already captured for the previous turn. Move unbound steering
    /// along with the new lead, leaving genuinely folded messages behind.
    pub(super) fn split_answered_turn(
        &mut self,
        lead: RequestId,
        next: RequestId,
        callback: &super::ProviderCallback,
    ) -> Result<bool, ModelError> {
        let previous = self
            .requests
            .get(&lead)
            .ok_or(ModelError::UnknownRequest(lead))?;
        let held = previous.pending_final.is_none();
        let Some(answer) = previous
            .pending_final
            .clone()
            .or_else(|| Self::held_interim(previous))
        else {
            // A running generation can be replaced before it answers (Cursor
            // does this for follow-ups). Its messages still await one answer.
            return Ok(false);
        };
        let boundary = previous.submission_boundary;
        for id in self.group_members(lead) {
            if let Some(member) = self.requests.get_mut(&id).filter(|member| {
                !member.settled()
                    && (member.id == next || (member.steered && !member.trusted_start_bound))
            }) {
                member.group = (id != next).then_some(next);
                member.submission_boundary = boundary;
            }
        }
        if held {
            self.deliver_held_interim(
                callback.agent_id,
                lead,
                answer,
                callback.occurred_at_ms,
                "background_shells_new_turn",
            );
        } else {
            self.complete_pending_final(callback.agent_id, callback.occurred_at_ms)?;
        }
        let member = self
            .requests
            .get_mut(&next)
            .ok_or(ModelError::UnknownRequest(next))?;
        member.phase = RequestPhase::Active;
        member.provider_session_id = callback.provider_session_id.clone();
        member.provider_turn_id = callback.provider_turn_id.clone();
        member.provider_prompt_id = callback.provider_prompt_id.clone();
        member.trusted_start_bound = true;
        member.uncertain_outcome = false;
        member.progress_at_ms = Some(callback.occurred_at_ms);
        let agent = self
            .agents
            .get_mut(&callback.agent_id)
            .ok_or(ModelError::UnknownAgent(callback.agent_id))?;
        member.submission_status_revision = agent.status_revision;
        agent.current_request = Some(next);
        Ok(true)
    }

    /// The interim text a `Stop` gave while this request held for its shells,
    /// when it belongs to the request's current provider session. A wake turn
    /// that ends without a `Stop` (Bus restarted under it, or Claude folded the
    /// next task notification into it) leaves it as the reply Bus captured.
    pub(super) fn held_interim(request: &Request) -> Option<PendingFinal> {
        request
            .shell_hold
            .as_ref()
            .filter(|hold| hold.interim.provider_session_id == request.provider_session_id)
            .map(|hold| hold.interim.clone())
    }

    /// The agent's current request when its turn ended without a reply and
    /// `settle_ended_request` will close it once the settle window passes.
    fn settle_candidate(&self, agent: AgentId) -> Option<&Request> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.deletion_pending
            || agent_state.session_binding_invalidated
            || agent_state.dialog
            || agent_state.status != RuntimeStatus::Idle
        {
            return None;
        }
        let request = self.requests.get(&agent_state.current_request?)?;
        (!request.awaiting_background
            && request.pending_final.is_none()
            && request.turn_ended_at_ms.is_some())
        .then_some(request)
    }

    /// Whether `request` (or the turn it joined) ended without a reply and
    /// awaits settlement, which attaches its reason (a provider error, say).
    /// Followers wait for that instead of taking the turn as done.
    pub(crate) fn settling(&self, request: &Request) -> bool {
        !request.settled()
            && self
                .settle_candidate(request.agent_id)
                .is_some_and(|lead| lead.id == request.group.unwrap_or(request.id))
    }

    /// Run after callback consumption, so a final spooled at the deadline wins.
    /// No reply is invented when capture failed; the sender sees `abandoned`
    /// with a reason: `provider_error: ...` when `provider_error` finds the
    /// error that ended the turn, else `reply_not_captured`. A reply captured
    /// earlier and held for the turn's shells is never lost that way: once
    /// `HELD_REPLY_SETTLE_MS` has passed with no late `Stop`, it is delivered
    /// with reason `background_shells_interim` (live, 2026-10-16: an
    /// orchestrator's wake turns ended without a `Stop`, under a Bus restart
    /// and under a folded task notification, and both groups of MASTER
    /// messages were abandoned with their captured replies).
    pub(crate) fn settle_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        provider_error: impl FnOnce(&Request) -> Option<String>,
    ) -> Option<RequestId> {
        let request = self.settle_candidate(agent)?;
        let id = request.id;
        let held = Self::held_interim(request);
        let settle_ms = if held.is_some() {
            HELD_REPLY_SETTLE_MS
        } else {
            TURN_SETTLE_MS
        };
        if now_ms < request.turn_ended_at_ms?.saturating_add(settle_ms) {
            return None;
        }
        let provider_error = provider_error(request);
        if let (None, Some(interim)) = (&provider_error, held) {
            return self.deliver_held_interim(
                agent,
                id,
                interim,
                now_ms,
                "background_shells_interim",
            );
        }
        let (reason, error) = match &provider_error {
            Some(message) => provider_error_outcome(message),
            None => (
                "reply_not_captured".to_owned(),
                "Provider turn ended without a captured reply; request closed. Inspect its terminal for the result.".to_owned(),
            ),
        };
        self.close_turn_with_reason(id, now_ms, reason).ok()?;
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

    /// Abandon `id`'s turn with `reason`. Messages typed or coalesced into
    /// this turn close with it and share its reason; members that already
    /// settled keep their own outcome.
    pub(super) fn close_turn_with_reason(
        &mut self,
        id: RequestId,
        now_ms: u64,
        reason: String,
    ) -> Result<(), ModelError> {
        let mut closed = self.group_members(id);
        closed.retain(|member| self.requests.get(member).is_some_and(|m| !m.settled()));
        closed.push(id);
        self.abandon_current_request(id, now_ms)?;
        for request in closed {
            if let Some(request) = self.requests.get_mut(&request) {
                request.failure_reason = Some(reason.clone());
            }
        }
        Ok(())
    }
}

/// The request reason and agent error for a turn a provider error ended:
/// `provider_error: ...` tells callers (and `message_provider_error`) to retry.
pub(super) fn provider_error_outcome(message: &str) -> (String, String) {
    (
        format!("provider_error: {message}"),
        format!("Provider turn failed: {message}; request closed. Resend it to retry."),
    )
}
