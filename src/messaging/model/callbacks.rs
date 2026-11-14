//! Callbacks operations for the messaging state.
use super::{
    AgentId, BusState, CallbackDisposition, CallbackEventKind, CallbackRejection, ModelError,
    PendingFinal, Provider, ProviderCallback, Request, RequestId, RequestPhase,
};

#[path = "callback_latency.rs"]
mod callback_latency;
mod kimi;

impl BusState {
    /// Report identity ignores incidental changes to the hook envelope. Keep
    /// these guards with callback keys so they persist and retire by launch.
    pub(crate) fn has_reported_final(&self, key: &str) -> bool {
        self.consumed_callbacks.contains_key(key)
    }

    pub(crate) fn record_reported_final(&mut self, key: String, launch: String) {
        std::sync::Arc::make_mut(&mut self.consumed_callbacks).insert(key, Some(launch));
    }

    fn match_owned_claude_paste(&self, callback: &mut ProviderCallback) {
        if !matches!(callback.kind, CallbackEventKind::PromptStarted) {
            return;
        }
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return;
        };
        if !agent.provider.uses_claude_harness() {
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
            request.matches_callback_payload(text, agent.provider)
                || self.group_members(request.id).iter().any(|id| {
                    self.requests.get(id).is_some_and(|member| {
                        member.steered
                            && matches!(
                                member.phase,
                                RequestPhase::Submitting | RequestPhase::Active
                            )
                            && member.matches_callback_payload(text, agent.provider)
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
        if self
            .consumed_callbacks
            .get(&callback.callback_id)
            .and_then(Option::as_ref)
            == Some(&callback.launch_id)
        {
            return CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback);
        }
        if std::sync::Arc::make_mut(&mut self.consumed_callbacks)
            .insert(
                callback.callback_id.clone(),
                Some(callback.launch_id.clone()),
            )
            .is_some()
        {
            return CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback);
        }
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        if agent.deletion_pending {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        }
        let provider = agent.provider;
        let turn_key = provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        );
        if turn_key
            .as_ref()
            .is_some_and(|key| self.retried_provider_turns.contains(key))
        {
            return CallbackDisposition::Rejected(CallbackRejection::DuplicateFinal);
        }
        if let Some(disposition) = self.accept_retry_input(&callback) {
            return disposition;
        }
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
                            && callback.prompt_payload.as_deref().is_some_and(|payload| {
                                request.matches_callback_payload(payload, provider)
                            })
                    });
            let bound = self
                .agents
                .get(&callback.agent_id)
                .and_then(|a| a.current_request)
                .and_then(|id| self.requests.get(&id))
                .is_some_and(|request| bound_callback_turn(request, &callback));
            if !absorbed && !bound {
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
        if let Some(request) = self.unbound_request_typed_before(callback) {
            request
                .foreign_turn_settled_at_ms
                .get_or_insert(callback.occurred_at_ms);
        }
    }

    /// Records that the agent started a turn of its own after Bus typed its
    /// unbound current request (live 2026-11-13: a Claude task notification
    /// made the agent Working, and a message steered into the unstarted
    /// request reached the model but was abandoned with it).
    fn note_foreign_turn_started(&mut self, callback: &ProviderCallback) {
        if let Some(request) = self.unbound_request_typed_before(callback) {
            request
                .foreign_turn_started_at_ms
                .get_or_insert(callback.occurred_at_ms);
        }
    }

    fn unbound_request_typed_before(
        &mut self,
        callback: &ProviderCallback,
    ) -> Option<&mut Request> {
        let request = self.unbound_request(callback.agent_id)?.id;
        self.requests.get_mut(&request).filter(|request| {
            request.expected_launch_id.as_deref() == Some(callback.launch_id.as_str())
                && request
                    .submission_boundary
                    .is_some_and(|boundary| callback.sequence > boundary)
        })
    }

    fn accept_callback_for_request(
        &mut self,
        mut callback: ProviderCallback,
        turn_key: Option<String>,
    ) -> CallbackDisposition {
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let provider = agent.provider;
        let Some(request_id) = agent.current_request else {
            if matches!(callback.kind, CallbackEventKind::PromptStarted) {
                self.unrelated_provider_turns.extend(turn_key);
            }
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let Some(request) = self.requests.get(&request_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
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
            self.note_foreign_turn_started(&callback);
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if self.is_unrelated_turn(&callback) {
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if self.apply_callback_steering(request_id, &mut callback, provider) {
            let _ = self.complete_pending_final(callback.agent_id, callback.occurred_at_ms);
            return CallbackDisposition::AcceptedSteering;
        }
        // Steering that did not match leaves the current request unchanged.
        let Some(request) = self.requests.get(&request_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        // Native question answers, direct input and task notifications can all
        // join an already bound turn. After matching Bus steering above, treat
        // the additional input as progress without rebinding or clearing its answer.
        if matches!(callback.kind, CallbackEventKind::PromptStarted)
            && bound_callback_turn(request, &callback)
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload, provider))
        {
            return CallbackDisposition::AcceptedProgress;
        }
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
                .is_some_and(|payload| !request.matches_callback_payload(payload, provider));
        if matches!(callback.kind, CallbackEventKind::PromptStarted)
            && !continuation
            && callback.provider_turn_id.is_some()
            && callback.provider_turn_id != request.provider_turn_id
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload, provider))
        {
            // A new turn of its own after one already finished: the typed
            // request is not coming, so the queue moves on.
            if !request.trusted_start_bound && request.foreign_turn_settled_at_ms.is_some() {
                self.release_unbound_request(callback.agent_id, callback.occurred_at_ms);
            }
            self.note_foreign_turn_started(&callback);
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if let Err(reason) = check_callback_turn(request, &callback, continuation, provider) {
            return CallbackDisposition::Rejected(reason);
        }

        self.apply_callback_kind(request_id, callback, turn_key, continuation, provider)
    }

    fn apply_callback_kind(
        &mut self,
        request_id: RequestId,
        callback: ProviderCallback,
        turn_key: Option<String>,
        continuation: bool,
        provider: Provider,
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
                        .is_some_and(|payload| request.matches_callback_payload(payload, provider))
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
                request.submission_error = None;
                request.pending_final = None;
                request.turn_ended_at_ms = None;
                request.awaiting_background = false;
                let restart_continuation = request.restart_continuation;
                if restart_continuation {
                    self.bind_restart_continuation_group(request_id);
                }
                if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
                    agent.actionable_error = None;
                }
                self.release_parked_prepared_retry(callback.agent_id, request_id);
                if continuation {
                    CallbackDisposition::AcceptedContinuation
                } else {
                    CallbackDisposition::AcceptedBinding
                }
            }
            CallbackEventKind::BackgroundPending => {
                if request.pending_final.is_some() {
                    return CallbackDisposition::Rejected(CallbackRejection::DuplicateFinal);
                }
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
                match self.complete_pending_final(callback.agent_id, callback.occurred_at_ms) {
                    Ok(true) => CallbackDisposition::AcceptedCompleted,
                    Ok(false) => CallbackDisposition::AcceptedPendingSettlement,
                    Err(_) => CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest),
                }
            }
            CallbackEventKind::Error { .. } => {
                self.apply_error_callback(request_id, callback, turn_key)
            }
        }
    }

    fn apply_callback_steering(
        &mut self,
        request_id: RequestId,
        callback: &mut ProviderCallback,
        provider: Provider,
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
            let request_turn_id = request.provider_turn_id.clone();
            let steered = self.group_members(request_id).into_iter().find(|member| {
                self.requests.get(member).is_some_and(|m| {
                    m.steered
                        && !m.trusted_start_bound
                        && matches!(m.phase, RequestPhase::Submitting | RequestPhase::Active)
                        && m.matches_callback_payload(payload, provider)
                })
            });
            if let Some(member) = steered {
                if let Some(member_state) = self.requests.get_mut(&member) {
                    member_state
                        .latency
                        .provider_started_at_ms
                        .get_or_insert(callback.occurred_at_ms);
                }
                if callback.provider_turn_id.is_some()
                    && callback.provider_turn_id != request_turn_id
                    && self
                        .split_answered_turn(request_id, member, callback)
                        .unwrap_or(false)
                {
                    return true;
                }
                if let Some(member) = self.requests.get_mut(&member) {
                    member.phase = RequestPhase::Active;
                    member.trusted_start_bound = true;
                    member.uncertain_outcome = false;
                    member.submission_error = None;
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

    /// An inherited notification is foreign unless its turn already belongs
    /// to a request, or its payload matches input Bus actually submitted.
    pub(crate) fn mark_unowned_background_turn(&mut self, callback: &ProviderCallback) {
        let Some(payload) = callback.prompt_payload.as_deref() else {
            return;
        };
        let lead = self
            .agents
            .get(&callback.agent_id)
            .and_then(|agent| agent.current_request.map(|lead| (lead, agent.provider)));
        if let Some((lead, provider)) = lead {
            let owned = std::iter::once(lead)
                .chain(self.group_members(lead))
                .any(|id| {
                    self.requests.get(&id).is_some_and(|request| {
                        (matches!(
                            request.phase,
                            RequestPhase::Submitting | RequestPhase::Active
                        ) && request.matches_callback_payload(payload, provider))
                            || (id == lead && bound_callback_turn(request, callback))
                    })
                });
            if owned {
                return;
            }
        }
        self.unrelated_provider_turns.extend(provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        ));
    }

    /// Settles the agent's current request, and every confirmed member of its
    /// group, when the turn ends. The final text is not a reply. An explicit
    /// `--reply-to` already stored on a request stays. Unconfirmed steering
    /// needs turn proof, with the completed lead retained as an anchor while
    /// the provider can still claim those inputs.
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
        let (members, unresolved): (Vec<_>, Vec<_>) = members.into_iter().partition(|member| {
            self.requests
                .get(member)
                .is_none_or(|m| !m.steered || m.trusted_start_bound || m.settled())
        });
        // A turn final is not a reply. An explicit --reply-to already stored
        // on the request stays; everyone else just becomes terminal.
        let turn_key = pending_turn_key(request, &pending);
        for member in &unresolved {
            if let Some(member) = self.requests.get_mut(member) {
                member.phase = RequestPhase::Submitting;
                member.uncertain_outcome = true;
                member.submission_error.get_or_insert_with(|| {
                    "Provider turn ended without claiming this input; awaiting turn ownership proof."
                        .into()
                });
            }
        }
        for member in members {
            if let Some(member) = self.requests.get_mut(&member) {
                if member.phase != RequestPhase::Abandoned {
                    member.completed_at_ms.get_or_insert(completed_at_ms);
                    member.phase = RequestPhase::Completed;
                }
                member.pending_final = None;
            }
        }
        let request = self
            .requests
            .get_mut(&request_id)
            .ok_or(ModelError::UnknownRequest(request_id))?;
        if request.phase != RequestPhase::Abandoned {
            request.completed_at_ms.get_or_insert(completed_at_ms);
            request.phase = RequestPhase::Completed;
        }
        if unresolved.is_empty() {
            request.pending_final = None;
        }
        if let Some(key) = turn_key {
            self.consumed_provider_turns.insert(key);
        }
        self.finish_completed_turn(agent_id, request_id, unresolved.is_empty())?;
        Ok(unresolved.is_empty())
    }

    fn finish_completed_turn(
        &mut self,
        agent_id: AgentId,
        request_id: RequestId,
        complete: bool,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if complete {
            agent.current_request = None;
            if agent
                .provider_retry
                .as_ref()
                .is_some_and(|h| h.lead == request_id)
            {
                agent.provider_retry = None;
            }
        }
        agent.actionable_error = None;
        Ok(())
    }
}

fn pending_turn_key(request: &Request, pending: &PendingFinal) -> Option<String> {
    provider_turn_key(
        request.expected_launch_id.as_deref().unwrap_or_default(),
        pending.provider_session_id.as_deref(),
        pending.provider_turn_id.as_deref(),
    )
}

fn bound_callback_turn(request: &Request, callback: &ProviderCallback) -> bool {
    (matches!(
        request.phase,
        RequestPhase::Submitting | RequestPhase::Active
    ) || request.pending_final.is_some())
        && request.trusted_start_bound
        && request.provider_session_id.is_some()
        && request.provider_turn_id.is_some()
        && request.provider_turn_id == callback.provider_turn_id
        && check_callback_identity(request, callback).is_ok()
        && !request
            .provider_prompt_id
            .as_deref()
            .is_some_and(|expected| {
                callback
                    .provider_prompt_id
                    .as_deref()
                    .is_some_and(|actual| actual != expected)
            })
}

pub(super) fn provider_turn_key(
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
    provider: Provider,
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
            .is_some_and(|payload| !request.matches_callback_payload(payload, provider))
    {
        return Err(CallbackRejection::WrongPrompt);
    }

    Ok(())
}
