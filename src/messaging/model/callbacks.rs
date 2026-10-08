//! Callbacks operations for the messaging state.
use super::{
    AgentId, BusState, CallbackDisposition, CallbackEventKind, CallbackRejection, ModelError,
    PendingFinal, ProviderCallback, Reply, RequestPhase, RuntimeStatus, STEERING_SETTLE_MS,
};
use crate::messaging::provider_glue::callbacks;

impl BusState {
    pub(crate) fn accept_callback(&mut self, callback: ProviderCallback) -> CallbackDisposition {
        let agent = callback.agent_id;
        let occurred_at_ms = callback.occurred_at_ms;
        let current = self.agents.get(&agent).and_then(|a| a.current_request);
        let disposition = self.accept_callback_unrecorded(callback);
        if !matches!(disposition, CallbackDisposition::Rejected(_)) {
            if let Some(request) = current.and_then(|id| self.requests.get_mut(&id)) {
                let progress = request.progress_at_ms.get_or_insert(occurred_at_ms);
                *progress = (*progress).max(occurred_at_ms);
            }
        }
        // A provider turn event on a Bus request proves the agent was busy
        // with it, even when the turn started and ended between two status
        // polls that only ever saw Idle. Recording the edge here lets
        // `turn_ended` (and so `send --async`) finish on the next Idle poll.
        if matches!(
            disposition,
            CallbackDisposition::AcceptedBinding
                | CallbackDisposition::AcceptedContinuation
                | CallbackDisposition::AcceptedSteering
                | CallbackDisposition::AcceptedProgress
                | CallbackDisposition::AcceptedPendingSettlement
        ) {
            self.record_busy_edge(agent);
        }
        disposition
    }

    /// Marks `agent` busy at a fresh status revision, as a Working poll would.
    pub(super) fn record_busy_edge(&mut self, agent: AgentId) {
        let revision = self.next_status_revision;
        self.next_status_revision = self.next_status_revision.saturating_add(1);
        if let Some(agent) = self.agents.get_mut(&agent) {
            agent.busy_revision = revision;
        }
    }

    fn accept_callback_unrecorded(&mut self, callback: ProviderCallback) -> CallbackDisposition {
        if !self
            .consumed_callback_ids
            .insert(callback.callback_id.clone())
        {
            return CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback);
        }
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        if agent.deletion_pending {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        }
        let turn_key = provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        );
        if let Some(key) = turn_key
            .as_ref()
            .filter(|key| self.unrelated_provider_turns.contains(*key))
        {
            // Text Bus typed while the agent ran a turn of its own can join
            // that turn: its submit hook then names that turn, which carries
            // the reply.
            let absorbed = matches!(callback.kind, CallbackEventKind::PromptStarted)
                && self
                    .unbound_request(callback.agent_id)
                    .is_some_and(|request| {
                        request.expected_launch_id.as_deref() == Some(callback.launch_id.as_str())
                            && request
                                .submission_boundary
                                .is_some_and(|boundary| callback.sequence > boundary)
                            && callback
                                .prompt_payload
                                .as_deref()
                                .is_some_and(|payload| request.matches_callback_payload(payload))
                    });
            if !absorbed {
                if matches!(
                    callback.kind,
                    CallbackEventKind::Final { .. } | CallbackEventKind::Error { .. }
                ) {
                    self.unrelated_provider_turns.remove(key);
                    self.note_foreign_turn_settled(&callback);
                }
                return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
            }
            self.unrelated_provider_turns.remove(key);
            return self.accept_callback_for_request(callback, turn_key);
        }
        self.accept_callback_for_request(callback, turn_key)
    }

    /// Records that a turn the agent ran on its own finished after Bus typed
    /// its unbound current request.
    fn note_foreign_turn_settled(&mut self, callback: &ProviderCallback) {
        let Some(request) = self.unbound_request(callback.agent_id).map(|r| r.id) else {
            return;
        };
        if let Some(request) = self.requests.get_mut(&request).filter(|request| {
            request.expected_launch_id.as_deref() == Some(callback.launch_id.as_str())
                && request
                    .submission_boundary
                    .is_some_and(|boundary| callback.sequence > boundary)
        }) {
            request
                .foreign_turn_settled_at_ms
                .get_or_insert(callback.occurred_at_ms);
        }
    }

    fn accept_callback_for_request(
        &mut self,
        callback: ProviderCallback,
        turn_key: Option<String>,
    ) -> CallbackDisposition {
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let Some(request_id) = agent.current_request else {
            if matches!(callback.kind, CallbackEventKind::PromptStarted) {
                self.unrelated_provider_turns.extend(turn_key);
            }
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let Some(request) = self.requests.get(&request_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let settled_after_submission = agent.status == RuntimeStatus::Idle
            && agent.status_revision > request.submission_status_revision;
        if request.expected_launch_id.as_deref() != Some(callback.launch_id.as_str()) {
            return CallbackDisposition::Rejected(CallbackRejection::WrongLaunch);
        }
        if request
            .submission_boundary
            .is_some_and(|boundary| callback.sequence <= boundary)
        {
            return CallbackDisposition::Rejected(CallbackRejection::BeforeSubmissionBoundary);
        }
        if let Some(expected) = request.provider_session_id.as_deref() {
            if callback.provider_session_id.as_deref() != Some(expected) {
                return CallbackDisposition::Rejected(CallbackRejection::WrongSession);
            }
        }
        // Cursor opens a generation of its own when a background shell finishes.
        // That prompt is a task notice, not a room message. Record the turn as
        // unrelated so its later stop is neither the room reply nor WrongTurn.
        if callback
            .prompt_payload
            .as_deref()
            .is_some_and(callbacks::cursor_reply::is_background_task_notice)
        {
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if self.is_unrelated_turn(&callback) {
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        // Input Bus typed into this turn binds where the provider reports it:
        // in the running turn, or in a turn of its own that then carries the
        // group's reply. It is never an unrelated turn or an agent error.
        if let (CallbackEventKind::PromptStarted, Some(payload), true) = (
            &callback.kind,
            callback.prompt_payload.as_deref(),
            request.trusted_start_bound,
        ) {
            let steered = self.group_members(request_id).into_iter().find(|member| {
                self.requests.get(member).is_some_and(|m| {
                    m.steered && !m.settled() && m.matches_callback_payload(payload)
                })
            });
            if let Some(member) = steered {
                if let Some(member) = self.requests.get_mut(&member) {
                    member.trusted_start_bound = true;
                    member.uncertain_outcome = false;
                    member.provider_session_id = callback.provider_session_id.clone();
                    member.provider_turn_id = callback.provider_turn_id.clone();
                }
                if let Some(lead) = self.requests.get_mut(&request_id) {
                    if callback.provider_turn_id.is_some()
                        && callback.provider_turn_id != lead.provider_turn_id
                    {
                        lead.provider_turn_id = callback.provider_turn_id;
                        lead.provider_prompt_id = callback.provider_prompt_id;
                        lead.pending_final = None;
                    }
                }
                return CallbackDisposition::AcceptedSteering;
            }
        }
        // A turn that wakes after the request paused for background work carries
        // the real reply. Once the request holds a final reply, a new turn is the
        // agent's own activity and must not replace or discard that reply.
        let continuation = matches!(callback.kind, CallbackEventKind::PromptStarted)
            && request.trusted_start_bound
            && request.pending_final.is_none()
            && request.provider_session_id.is_some()
            && callback.provider_session_id == request.provider_session_id
            && request.provider_turn_id.is_some()
            && callback.provider_turn_id.is_some()
            && callback.provider_turn_id != request.provider_turn_id
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload));
        if matches!(callback.kind, CallbackEventKind::PromptStarted)
            && !continuation
            && callback.provider_turn_id.is_some()
            && callback.provider_turn_id != request.provider_turn_id
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload))
        {
            // A new turn of its own after one already finished: the typed
            // request is not coming, so the queue moves on.
            if !request.trusted_start_bound && request.foreign_turn_settled_at_ms.is_some() {
                self.release_unbound_request(callback.agent_id, callback.occurred_at_ms);
            }
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if let Some(expected) = request.provider_turn_id.as_deref() {
            if !continuation && callback.provider_turn_id.as_deref() != Some(expected) {
                return CallbackDisposition::Rejected(CallbackRejection::WrongTurn);
            }
        }
        if let Some(expected) = request.provider_prompt_id.as_deref() {
            if !continuation
                && callback
                    .provider_prompt_id
                    .as_deref()
                    .is_some_and(|actual| actual != expected)
            {
                return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
            }
        }
        if !continuation
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload))
        {
            return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
        }

        match callback.kind {
            CallbackEventKind::PromptStarted => {
                if callback.provider_session_id.is_none() || callback.provider_turn_id.is_none() {
                    return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
                }
                if !continuation
                    && !callback
                        .prompt_payload
                        .as_deref()
                        .is_some_and(|payload| request.matches_callback_payload(payload))
                {
                    return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
                }
                let Some(request) = self.requests.get_mut(&request_id) else {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                };
                request.provider_session_id = callback.provider_session_id;
                request.provider_turn_id = callback.provider_turn_id;
                request.provider_prompt_id = callback.provider_prompt_id;
                request.trusted_start_bound = true;
                request.phase = RequestPhase::Active;
                request.uncertain_outcome = false;
                request.pending_final = None;
                if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
                    agent.actionable_error = None;
                }
                if continuation {
                    CallbackDisposition::AcceptedContinuation
                } else {
                    CallbackDisposition::AcceptedBinding
                }
            }
            CallbackEventKind::BackgroundPending => {
                if !request.trusted_start_bound
                    || request.provider_session_id.is_none()
                    || request.provider_turn_id.is_none()
                {
                    return CallbackDisposition::Rejected(CallbackRejection::UnboundFinal);
                }
                let Some(request) = self.requests.get_mut(&request_id) else {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                };
                request.pending_final = None;
                CallbackDisposition::AcceptedProgress
            }
            CallbackEventKind::Final { text } => {
                if !request.trusted_start_bound
                    || request.provider_session_id.is_none()
                    || request.provider_turn_id.is_none()
                {
                    return CallbackDisposition::Rejected(CallbackRejection::UnboundFinal);
                }
                if request.pending_final.is_some() {
                    return CallbackDisposition::Rejected(CallbackRejection::DuplicateFinal);
                }
                if turn_key
                    .as_ref()
                    .is_some_and(|key| self.consumed_provider_turns.contains(key))
                {
                    return CallbackDisposition::Rejected(CallbackRejection::DuplicateFinal);
                }
                let Some(request) = self.requests.get_mut(&request_id) else {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                };
                request.pending_final = Some(PendingFinal {
                    callback_id: callback.callback_id,
                    text,
                    received_at_ms: callback.occurred_at_ms,
                    provider_session_id: callback.provider_session_id,
                    provider_turn_id: callback.provider_turn_id,
                });
                if settled_after_submission {
                    match self.complete_pending_final(callback.agent_id, callback.occurred_at_ms) {
                        Ok(true) => CallbackDisposition::AcceptedCompleted,
                        Ok(false) => CallbackDisposition::AcceptedPendingSettlement,
                        Err(_) => CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest),
                    }
                } else {
                    CallbackDisposition::AcceptedPendingSettlement
                }
            }
            CallbackEventKind::Error { message } => {
                if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
                    agent.actionable_error = Some(message);
                }
                CallbackDisposition::AcceptedError
            }
        }
    }

    /// Whether the callback belongs to a turn the agent started on its own.
    pub(crate) fn is_unrelated_turn(&self, callback: &ProviderCallback) -> bool {
        provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        )
        .is_some_and(|key| self.unrelated_provider_turns.contains(&key))
    }

    /// Settles the agent's current request, and every request in its group,
    /// with its pending final reply. Returns whether it settled: a group with
    /// typed input whose submit hook has not arrived waits a few seconds, in
    /// case the provider runs that input as a turn of its own.
    pub(super) fn complete_pending_final(
        &mut self,
        agent_id: AgentId,
        completed_at_ms: u64,
    ) -> Result<bool, ModelError> {
        let request_id = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?
            .current_request;
        let Some(request_id) = request_id else {
            return Ok(false);
        };
        let request = self
            .requests
            .get(&request_id)
            .ok_or(ModelError::UnknownRequest(request_id))?;
        let Some(pending) = request.pending_final.clone() else {
            return Ok(false);
        };
        let members = self.group_members(request_id);
        let unseen_steering = members.iter().any(|member| {
            self.requests
                .get(member)
                .is_some_and(|m| m.steered && !m.trusted_start_bound && !m.settled())
        });
        if unseen_steering
            && completed_at_ms < pending.received_at_ms.saturating_add(STEERING_SETTLE_MS)
        {
            return Ok(false);
        }
        let room_id = request.room_id;
        let delivery_only = request.delivery_only();
        let turn_key = provider_turn_key(
            request.expected_launch_id.as_deref().unwrap_or_default(),
            pending.provider_session_id.as_deref(),
            pending.provider_turn_id.as_deref(),
        );
        let reply = Reply {
            request_id,
            agent_id,
            text: pending.text.clone(),
            received_at_ms: pending.received_at_ms,
        };
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(ModelError::UnknownRoom(room_id))?;
        // The reply to a delivery-only dialog notice is as hidden as the notice.
        if !delivery_only {
            room.latest_replies.insert(agent_id, reply);
            if self.visible_room != Some(room_id) {
                room.unread_count = room.unread_count.saturating_add(1);
            }
        }
        for member in members {
            if let Some(member) = self.requests.get_mut(&member).filter(|m| !m.settled()) {
                member.phase = RequestPhase::Completed;
                member.completed_at_ms = Some(completed_at_ms);
                member.pending_final = Some(pending.clone());
            }
        }
        let request = self
            .requests
            .get_mut(&request_id)
            .ok_or(ModelError::UnknownRequest(request_id))?;
        request.phase = RequestPhase::Completed;
        request.completed_at_ms = Some(completed_at_ms);
        if let Some(key) = turn_key {
            self.consumed_provider_turns.insert(key);
        }
        let agent = self
            .agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        agent.current_request = None;
        agent.actionable_error = None;
        Ok(true)
    }
}

fn provider_turn_key(
    launch_id: &str,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> Option<String> {
    Some(format!("{launch_id}\u{0}{}\u{0}{}", session_id?, turn_id?))
}
