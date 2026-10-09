//! Callbacks operations for the messaging state.
use super::{
    AgentId, BusState, CallbackDisposition, CallbackEventKind, CallbackRejection, ModelError,
    PendingFinal, ProviderCallback, Reply, Request, RequestId, RequestPhase, RuntimeStatus,
    STEERING_SETTLE_MS,
};

impl BusState {
    pub(crate) fn accept_callback(
        &mut self,
        mut callback: ProviderCallback,
    ) -> CallbackDisposition {
        self.match_owned_claude_paste(&mut callback);
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

    fn match_owned_claude_paste(&self, callback: &mut ProviderCallback) {
        if !matches!(callback.kind, CallbackEventKind::PromptStarted) {
            return;
        }
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return;
        };
        if agent.provider != super::Provider::ClaudeCode {
            return;
        }
        let Some(request) = agent.current_request.and_then(|id| self.requests.get(&id)) else {
            return;
        };
        let candidate = callback
            .prompt_payload
            .as_deref()
            .and_then(crate::agents::providers::claude_code::hooks::claude_paste_candidate);
        if let Some(candidate) = candidate.filter(|text| {
            request.matches_callback_payload(text)
                || self.group_members(request.id).iter().any(|id| {
                    self.requests.get(id).is_some_and(|member| {
                        member.steered && !member.settled() && member.matches_callback_payload(text)
                    })
                })
        }) {
            callback.prompt_payload = Some(candidate);
        }
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
        mut callback: ProviderCallback,
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
        if let Err(reason) = check_callback_identity(request, &callback) {
            return CallbackDisposition::Rejected(reason);
        }
        // Cursor opens a generation of its own when a background shell finishes.
        // That prompt is a task notice, not a room message. Record the turn as
        // unrelated so its later stop is neither the room reply nor WrongTurn.
        if callback
            .prompt_payload
            .as_deref()
            .is_some_and(crate::agents::providers::cursor::final_reply::is_background_task_notice)
        {
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if self.is_unrelated_turn(&callback) {
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if self.apply_callback_steering(request_id, &mut callback) {
            return CallbackDisposition::AcceptedSteering;
        }
        // Steering that did not match leaves the current request unchanged.
        let Some(request) = self.requests.get(&request_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        // A turn that wakes after the request paused for background work carries
        // the real reply. Once the request holds a final reply, a new turn is the
        // agent's own activity and must not replace or discard that reply.
        let continuation = matches!(callback.kind, CallbackEventKind::PromptStarted)
            && request.trusted_start_bound
            && request.pending_final.is_none()
            && request.turn_ended_at_ms.is_none()
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
        if let Err(reason) = check_callback_turn(request, &callback, continuation) {
            return CallbackDisposition::Rejected(reason);
        }

        self.apply_callback_kind(
            request_id,
            callback,
            turn_key,
            continuation,
            settled_after_submission,
        )
    }

    fn apply_callback_kind(
        &mut self,
        request_id: RequestId,
        callback: ProviderCallback,
        turn_key: Option<String>,
        continuation: bool,
        settled_after_submission: bool,
    ) -> CallbackDisposition {
        let Some(request) = self.requests.get(&request_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
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
                request.turn_ended_at_ms = None;
                request.awaiting_background = false;
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
                request.turn_ended_at_ms = None;
                request.awaiting_background = true;
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
                request.awaiting_background = false;
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
            CallbackEventKind::Error { .. } => {
                self.apply_error_callback(request_id, callback, turn_key)
            }
        }
    }

    fn apply_error_callback(
        &mut self,
        request_id: RequestId,
        callback: ProviderCallback,
        turn_key: Option<String>,
    ) -> CallbackDisposition {
        let CallbackEventKind::Error { message } = callback.kind else {
            return CallbackDisposition::Rejected(CallbackRejection::WrongTurn);
        };
        if self
            .requests
            .get(&request_id)
            .is_some_and(|r| r.trusted_start_bound)
        {
            if self
                .abandon_current_request(request_id, callback.occurred_at_ms)
                .is_err()
            {
                return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
            }
            self.consumed_provider_turns.extend(turn_key);
        }
        if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
            agent.actionable_error = Some(message);
        }
        CallbackDisposition::AcceptedError
    }

    fn apply_callback_steering(
        &mut self,
        request_id: RequestId,
        callback: &mut ProviderCallback,
    ) -> bool {
        let Some(request) = self.requests.get(&request_id) else {
            return false;
        };
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
                    lead.turn_ended_at_ms = None;
                    lead.awaiting_background = false;
                    if callback.provider_turn_id.is_some()
                        && callback.provider_turn_id != lead.provider_turn_id
                    {
                        lead.provider_turn_id = callback.provider_turn_id.take();
                        lead.provider_prompt_id = callback.provider_prompt_id.take();
                        lead.pending_final = None;
                    }
                }
                return true;
            }
        }
        false
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
        let turn_key = provider_turn_key(
            request.expected_launch_id.as_deref().unwrap_or_default(),
            pending.provider_session_id.as_deref(),
            pending.provider_turn_id.as_deref(),
        );
        self.publish_group_reply(request_id, &members, &pending)?;
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

    /// One shared turn can answer messages in a work room and MASTER. Publish
    /// once per visible room, under that room's newest participating request.
    fn publish_group_reply(
        &mut self,
        lead: RequestId,
        members: &[RequestId],
        pending: &PendingFinal,
    ) -> Result<(), ModelError> {
        let mut replies = std::collections::BTreeMap::new();
        for id in std::iter::once(&lead).chain(members) {
            let request = self
                .requests
                .get(id)
                .ok_or(ModelError::UnknownRequest(*id))?;
            if request.delivery_only() || request.settled() {
                continue;
            }
            replies.insert(
                request.room_id,
                Reply {
                    request_id: *id,
                    agent_id: request.agent_id,
                    text: pending.text.clone(),
                    received_at_ms: pending.received_at_ms,
                },
            );
        }
        for (id, reply) in replies {
            let room = self.rooms.get_mut(&id).ok_or(ModelError::UnknownRoom(id))?;
            room.latest_replies.insert(reply.agent_id, reply);
            if self.visible_room != Some(id) {
                room.unread_count = room.unread_count.saturating_add(1);
            }
        }
        Ok(())
    }
}

fn provider_turn_key(
    launch_id: &str,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> Option<String> {
    Some(format!("{launch_id}\u{0}{}\u{0}{}", session_id?, turn_id?))
}

fn check_callback_identity(
    request: &Request,
    callback: &ProviderCallback,
) -> Result<(), CallbackRejection> {
    if request.expected_launch_id.as_deref() != Some(callback.launch_id.as_str()) {
        return Err(CallbackRejection::WrongLaunch);
    }
    if request
        .submission_boundary
        .is_some_and(|boundary| callback.sequence <= boundary)
    {
        return Err(CallbackRejection::BeforeSubmissionBoundary);
    }
    if let Some(expected) = request.provider_session_id.as_deref() {
        if callback.provider_session_id.as_deref() != Some(expected) {
            return Err(CallbackRejection::WrongSession);
        }
    }
    Ok(())
}

fn check_callback_turn(
    request: &Request,
    callback: &ProviderCallback,
    continuation: bool,
) -> Result<(), CallbackRejection> {
    if let Some(expected) = request.provider_turn_id.as_deref() {
        if !continuation && callback.provider_turn_id.as_deref() != Some(expected) {
            return Err(CallbackRejection::WrongTurn);
        }
    }
    if let Some(expected) = request.provider_prompt_id.as_deref() {
        if !continuation
            && callback
                .provider_prompt_id
                .as_deref()
                .is_some_and(|actual| actual != expected)
        {
            return Err(CallbackRejection::WrongPrompt);
        }
    }
    if !continuation
        && callback
            .prompt_payload
            .as_deref()
            .is_some_and(|payload| !request.matches_callback_payload(payload))
    {
        return Err(CallbackRejection::WrongPrompt);
    }

    Ok(())
}
