//! `agent interrupt`: the provider's stop key was pressed, so the running turn
//! closes instead of running on. A message that never joined that turn stays
//! queued for the next one.
use super::{
    AgentId, AgentRuntimeIdentity, Author, BusState, ModelError, Prompt, PromptId, RequestId,
    RequestPhase, RoomId, RuntimeStatus,
};
use serde::{Deserialize, Serialize};

/// The reason an interrupted request closed with, which `wait` reports as
/// `message_interrupted`.
pub(crate) const INTERRUPTED: &str = "interrupted";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SendInterrupt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) activity: Option<(u64, u64)>,
    pub(crate) identity: AgentRuntimeIdentity,
    #[serde(default)]
    pub(crate) submission_boundary: Option<u64>,
    #[serde(default)]
    pub(crate) submission_status_revision: u64,
    #[serde(default)]
    pub(crate) provider_turn_id: Option<String>,
}

/// Durable autonomous activity and any stop that may already have been written.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AutonomousStopState {
    pub(super) provider_turn_id: Option<String>,
    pub(super) activity: Option<(u64, u64)>,
    #[serde(default)]
    pub(super) turn_activity: Option<(u64, u64)>,
    pub(super) claim: Option<SendInterrupt>,
}

impl AutonomousStopState {
    pub(super) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn clear_activity(&mut self) {
        self.provider_turn_id = None;
        self.activity = None;
        self.turn_activity = None;
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct SendInterruptOutcome {
    pub(crate) outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

impl SendInterrupt {
    pub(crate) fn replayable(&self) -> bool {
        self.target.is_some()
            || (self.activity.is_some()
                && self.identity.session_id.is_some()
                && self.provider_turn_id.is_some())
    }

    pub(crate) fn same_turn(&self, observed: &Self) -> bool {
        self.target == observed.target
            && self.submission_boundary == observed.submission_boundary
            && self.submission_status_revision == observed.submission_status_revision
            && self.identity.launch_id == observed.identity.launch_id
            && self.identity.terminal_id == observed.identity.terminal_id
            && self
                .identity
                .session_id
                .as_ref()
                .is_none_or(|session| observed.identity.session_id.as_ref() == Some(session))
            && self
                .provider_turn_id
                .as_ref()
                .is_none_or(|turn| observed.provider_turn_id.as_ref() == Some(turn))
    }
}

impl BusState {
    pub(crate) fn send_interrupt_target(&self, recipient: AgentId) -> Option<SendInterrupt> {
        let owner = self.agent(recipient)?;
        if owner.interrupt_pending {
            return None;
        }
        let target = owner.current_request.and_then(|id| self.request(id));
        if target.is_none() && !self.needs_interrupt_stop(owner) {
            return None;
        }
        Some(SendInterrupt {
            target: target.map(|request| request.id),
            activity: owner.autonomous_stop.activity.filter(|activity| {
                target.is_none() && owner.autonomous_stop.turn_activity.as_ref() == Some(activity)
            }),
            identity: owner.runtime_identity.clone(),
            submission_boundary: target.and_then(|row| row.submission_boundary),
            submission_status_revision: target.map_or(0, |row| row.submission_status_revision),
            provider_turn_id: target.map_or_else(
                || owner.autonomous_stop.provider_turn_id.clone(),
                |row| row.provider_turn_id.clone(),
            ),
        })
    }

    pub(crate) fn observe_stop_activity(&mut self, id: AgentId, activity: Option<(u64, u64)>) {
        if let Some(owner) = self.agents.get_mut(&id) {
            owner.autonomous_stop.activity = activity;
        }
    }

    /// A coherent native idle sample ends the observed episode. A bound claim
    /// can still deduplicate if its authenticated provider turn reappears.
    pub(crate) fn observe_autonomous_idle(
        &mut self,
        id: AgentId,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let Some(owner) = self
            .agents
            .get_mut(&id)
            .filter(|agent| agent.current_request.is_none())
        else {
            return Ok(());
        };
        let matches_unbound_claim = owner.autonomous_stop.claim.as_ref().is_some_and(|claim| {
            claim.target.is_none()
                && claim.provider_turn_id.is_none()
                && claim.identity.launch_id == owner.runtime_identity.launch_id
                && claim.identity.terminal_id == owner.runtime_identity.terminal_id
                && claim.identity.session_id.as_ref().is_none_or(|session| {
                    owner.runtime_identity.session_id.as_ref() == Some(session)
                })
        });
        owner.autonomous_stop.clear_activity();
        if matches_unbound_claim {
            owner.autonomous_stop.claim = None;
        }
        if owner.interrupt_pending {
            self.close_interrupted(id, now_ms)?;
        }
        Ok(())
    }

    pub(crate) fn autonomous_callback_turn(
        &self,
        callback: &super::ProviderCallback,
    ) -> Option<String> {
        let owner = self.agent(callback.agent_id)?;
        if owner.current_request.is_some()
            || owner.runtime_identity.launch_id.as_deref() != Some(&callback.launch_id)
            || owner.runtime_identity.session_id.is_none()
            || owner.runtime_identity.session_id != callback.provider_session_id
        {
            return None;
        }
        callback
            .provider_turn_id
            .clone()
            .filter(|turn| !turn.is_empty())
    }

    pub(crate) fn observe_autonomous_turn(&mut self, id: AgentId, turn: String, started: bool) {
        if let Some(owner) = self.agents.get_mut(&id) {
            let activity = &mut owner.autonomous_stop;
            if started {
                activity.provider_turn_id = Some(turn);
                activity.turn_activity = activity.activity;
            } else if activity.provider_turn_id.as_ref() == Some(&turn) {
                activity.provider_turn_id = None;
                activity.turn_activity = None;
                if activity.claim.as_ref().is_some_and(|claim| {
                    claim
                        .provider_turn_id
                        .as_ref()
                        .is_none_or(|claimed| claimed == &turn)
                }) {
                    activity.claim = None;
                }
            }
        }
    }

    pub(crate) fn stage_send_interrupt(
        &mut self,
        id: RequestId,
        interrupt: SendInterrupt,
    ) -> Result<(), ModelError> {
        self.requests
            .get_mut(&id)
            .ok_or(ModelError::UnknownRequest(id))?
            .send_interrupt = Some(interrupt);
        Ok(())
    }

    pub(crate) fn record_send_interrupt_progress(
        &mut self,
        request: RequestId,
        claim: bool,
        outcome: SendInterruptOutcome,
    ) -> Result<(), ModelError> {
        let row = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let recipient = row.agent_id;
        let intent = row.send_interrupt.take();
        row.send_interrupt_outcome = Some(outcome);
        if let Some(intent) = intent.filter(|_| claim) {
            if let Some(slot) = self.send_interrupt_claim_mut(recipient, intent.target) {
                *slot = Some(intent);
            }
        }
        Ok(())
    }

    pub(crate) fn send_interrupt_already_claimed(
        &self,
        recipient: AgentId,
        intent: &SendInterrupt,
    ) -> bool {
        let claim = match intent.target {
            Some(id) => self
                .request(id)
                .and_then(|row| row.send_interrupt_claim.as_ref()),
            None => self
                .agent(recipient)
                .and_then(|agent| agent.autonomous_stop.claim.as_ref()),
        };
        claim.is_some_and(|claim| claim.same_turn(intent))
    }

    pub(crate) fn release_send_interrupt_claim(
        &mut self,
        recipient: AgentId,
        intent: &SendInterrupt,
    ) {
        if let Some(slot) = self
            .send_interrupt_claim_mut(recipient, intent.target)
            .filter(|slot| slot.as_ref() == Some(intent))
        {
            *slot = None;
        }
    }

    fn send_interrupt_claim_mut(
        &mut self,
        recipient: AgentId,
        target: Option<RequestId>,
    ) -> Option<&mut Option<SendInterrupt>> {
        match target {
            Some(id) => self
                .requests
                .get_mut(&id)
                .map(|row| &mut row.send_interrupt_claim),
            None => self
                .agents
                .get_mut(&recipient)
                .map(|agent| &mut agent.autonomous_stop.claim),
        }
    }

    pub(crate) fn stamp_native_delivery_epoch(
        &mut self,
        id: RequestId,
        epoch: super::DeliveryEpoch,
        boundary: Option<u64>,
    ) -> Result<(), ModelError> {
        let request = self.request(id).ok_or(ModelError::UnknownRequest(id))?;
        let agent = self
            .agent(request.agent_id)
            .ok_or(ModelError::UnknownAgent(request.agent_id))?;
        let identity = &agent.runtime_identity;
        let origin = (agent.provider.uses_claude_harness()
            && identity.pane_id.as_ref() == Some(&epoch.pane_id)
            && identity.terminal_id.as_ref() == Some(&epoch.terminal_id)
            && identity.launch_id == request.expected_launch_id)
            .then(|| identity.clone());
        for member in std::iter::once(id).chain(self.group_members(id)) {
            let request = self
                .requests
                .get_mut(&member)
                .ok_or(ModelError::UnknownRequest(member))?;
            request.native_delivery_identity = origin.clone();
            request.native_delivery_callback_boundary = boundary.or(request.submission_boundary);
        }
        self.stamp_delivery_epoch(id, epoch)
    }

    /// Keep later execution evidence in the spool while the original input
    /// owns an unresolved retirement.
    pub(crate) fn defer_during_interrupt_retirement(
        &self,
        callback: &super::ProviderCallback,
    ) -> bool {
        self.interrupt_retirement_evidence_owner(callback.agent_id)
            .and_then(|lead| self.request(lead))
            .is_some_and(|lead| {
                lead.trusted_start_bound
                    && callback.provider_turn_id.is_some()
                    && callback.provider_turn_id != lead.provider_turn_id
                    && matches!(
                        callback.kind,
                        super::CallbackEventKind::PromptStarted
                            | super::CallbackEventKind::Final { .. }
                            | super::CallbackEventKind::BackgroundPending
                    )
            })
    }

    /// Screen status is a presentation observation, not proof that a bound turn
    /// stopped. An empty composer can be visible while foreground tools still run.
    /// A native final releases the current request; a background-pending Stop has
    /// already ended foreground work and needs only the ordinary request cleanup.
    pub(crate) fn needs_interrupt_stop(&self, agent: &super::RoomAgent) -> bool {
        if self.interrupt_retirement_owner(agent.id).is_some() {
            return matches!(
                agent.activity_status(),
                RuntimeStatus::Idle | RuntimeStatus::Working | RuntimeStatus::Blocked
            );
        }
        match agent.activity_status() {
            RuntimeStatus::Working | RuntimeStatus::Blocked => true,
            RuntimeStatus::Idle
                if agent
                    .provider_retry
                    .as_ref()
                    .is_some_and(|h| agent.current_request.is_none_or(|id| id == h.lead)) =>
            {
                false
            }
            RuntimeStatus::Idle => agent
                .current_request
                .and_then(|id| self.request(id))
                .is_some_and(|request| {
                    request.trusted_start_bound
                        && request.phase != RequestPhase::Abandoned
                        && request.pending_final.is_none()
                        && !request.awaiting_background
                }),
            _ => false,
        }
    }

    pub(crate) fn set_interrupt_retirement(
        &mut self,
        agent: AgentId,
        pending: bool,
    ) -> Result<(), ModelError> {
        let owner = self
            .agents
            .get(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        if let Some(request) = owner
            .current_request
            .and_then(|id| self.requests.get_mut(&id))
        {
            request.interrupt_retirement_pending = pending;
            if pending {
                if request.interrupt_retirement_identity.is_none() {
                    request.interrupt_retirement_identity =
                        request.native_delivery_identity.clone();
                }
            } else {
                request.interrupt_retirement_identity = None;
            }
            if pending {
                request.submission_error = Some(
                    "Claude interrupt draft retirement is unresolved; request remains tracked"
                        .into(),
                );
                request.turn_ended_at_ms = None;
            } else {
                request.submission_error = None;
            }
        }
        Ok(())
    }

    /// Resolve only after native evidence proves the original provider cannot execute its draft.
    pub(crate) fn resolve_exited_interrupt_terminal(
        &mut self,
        request: RequestId,
        identity: &super::AgentRuntimeIdentity,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let lead = self
            .request(request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let agent = lead.agent_id;
        let owner = self.agent(agent).ok_or(ModelError::UnknownAgent(agent))?;
        if !lead.interrupt_retirement_pending
            || owner.current_request != Some(request)
            || lead.interrupt_retirement_identity.as_ref() != Some(identity)
        {
            return Err(ModelError::InvalidTransition);
        }
        let retiring_current = owner.runtime_identity.terminal_id == identity.terminal_id;
        if retiring_current {
            let mut detached = owner.runtime_identity.clone();
            detached.pane_id = None;
            self.set_agent_runtime_identity(agent, detached)?;
        }
        self.set_interrupt_retirement(agent, false)?;
        self.clear_native_interrupt_wait(agent)?;
        self.complete_pending_final(agent, now_ms)?;
        let mut outstanding = self.group_members(request);
        outstanding.push(request);
        for id in outstanding {
            if let Some(row) = self.requests.get_mut(&id).filter(|row| !row.settled()) {
                row.uncertain_outcome = true;
                row.failure_reason = Some("interrupt_outcome_unknown".into());
            }
        }
        // Preserve a completed lead and its answer while closing unclaimed members.
        if self.agent(agent).and_then(|owner| owner.current_request) == Some(request) {
            let answered = self.request(request).filter(|row| row.settled()).cloned();
            self.abandon_current_request(request, now_ms)?;
            if let Some(answered) = answered {
                self.requests.insert(request, answered);
            }
        }
        self.requests
            .get_mut(&request)
            .expect("retirement owner")
            .interrupt_retirement_resolved = true;
        if retiring_current {
            self.observe_status(agent, super::RuntimeStatus::Unavailable, now_ms)?;
        }
        Ok(())
    }

    pub(crate) fn interrupt_retirement_origin(
        &self,
        id: RequestId,
    ) -> Option<&super::AgentRuntimeIdentity> {
        let lead = self.request(id)?;
        let identity = lead.native_delivery_identity.as_ref()?;
        if lead.interrupt_retirement_identity.as_ref() != Some(identity)
            || self.group_members(id).into_iter().any(|member| {
                self.request(member).is_some_and(|request| {
                    !request.settled()
                        && request.native_delivery_identity.as_ref() != Some(identity)
                })
            })
        {
            return None;
        }
        Some(identity)
    }

    pub(crate) fn interrupt_retirement_evidence_owner(&self, agent: AgentId) -> Option<RequestId> {
        let owner = self.agent(agent)?;
        let id = owner.current_request?;
        let request = self.request(id)?;
        (request.interrupt_retirement_pending
            || (owner.interrupt_pending
                && owner.provider.uses_claude_harness()
                && request.native_delivery_identity.is_some()))
        .then_some(id)
    }

    pub(crate) fn interrupt_retirement_owner(&self, agent: AgentId) -> Option<RequestId> {
        let id = self.agent(agent)?.current_request?;
        self.request(id)?.interrupt_retirement_pending.then_some(id)
    }

    /// A native result supersedes an older visual wait without asserting that
    /// any turn, request or terminal has completed.
    pub(crate) fn clear_native_interrupt_wait(&mut self, agent: AgentId) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .interrupt_pending = false;
        Ok(())
    }

    /// Apply positive native completion without changing the terminal's
    /// presentation status. The caller fences the observed turn and obligation.
    pub(crate) fn complete_native_interrupt(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        self.close_interrupted(agent, now_ms)?;
        self.complete_pending_final(agent, now_ms)?;
        self.note_turn_end(agent, now_ms);
        Ok(())
    }

    /// Records that the stop key went to `agent`'s terminal. The turn closes
    /// when Bus next sees the agent idle, not before: the provider decides.
    pub(crate) fn begin_interrupt(&mut self, agent: AgentId) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .interrupt_pending = true;
        Ok(())
    }

    /// Closes `agent`'s open turn, and every message typed into it, as
    /// interrupted. A message that is still queued and never joined the turn
    /// stays queued for the next one. A blocked-dialog notice, a restart
    /// continuation of this turn, and a provider-retry hold, including every
    /// request that hold has already prepared, close with the turn so they
    /// are not replayed. A reply captured before the stop took
    /// effect means the turn ended on its own first: that reply still
    /// completes it, but not the messages typed in that no hook placed in
    /// that turn. The agent ran those after the reply, and the stop ended them.
    pub(crate) fn close_interrupted(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        if self
            .agent(agent)
            .and_then(|a| a.current_request)
            .and_then(|id| self.request(id))
            .is_some_and(|r| r.interrupt_retirement_pending)
        {
            return Ok(Vec::new());
        }
        let agent_state = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent_state.interrupt_pending = false;
        if agent_state.current_request.is_none() {
            agent_state.autonomous_stop.clear_activity();
        }
        let hold = agent_state.provider_retry.take();
        let held = hold.as_ref().map(|retry| retry.lead);
        let prepared = hold.map(|retry| retry.queued_ids).unwrap_or_default();
        let current = agent_state.current_request;
        let mut closed = Vec::new();
        let replied = current
            .and_then(|id| self.requests.get(&id))
            .is_some_and(|request| request.pending_final.is_some());
        if let Some(current) = current.filter(|_| !replied) {
            self.close_turn_with_reason(current, now_ms, INTERRUPTED.into())?;
            closed.push(current);
        }
        if let Some(current) = current.filter(|_| replied) {
            for member in self.group_members(current) {
                let Some(request) = self
                    .requests
                    .get_mut(&member)
                    .filter(|m| m.steered && !m.trusted_start_bound && !m.settled())
                else {
                    continue;
                };
                request.group = None;
                request.phase = RequestPhase::Abandoned;
                request.completed_at_ms = Some(now_ms);
                request.failure_reason = Some(INTERRUPTED.into());
                closed.push(member);
            }
        }
        if let Some(held) = held.filter(|id| {
            Some(*id) != current
                && self
                    .request(*id)
                    .is_some_and(|r| r.phase != RequestPhase::Queued)
        }) {
            let outstanding = self.request(held).is_some_and(|r| !r.settled())
                || self
                    .group_members(held)
                    .iter()
                    .any(|id| self.request(*id).is_some_and(|r| !r.settled()));
            if outstanding {
                self.close_turn_with_reason(held, now_ms, INTERRUPTED.into())?;
                if replied {
                    self.agents
                        .get_mut(&agent)
                        .ok_or(ModelError::UnknownAgent(agent))?
                        .current_request = current;
                }
                closed.push(held);
            }
        }
        for queued in self.queued_requests(agent).to_vec() {
            if !self.queued_request_stops_with_the_turn(queued, current, held, &prepared) {
                continue;
            }
            self.recover_idle_request(queued, now_ms)?;
            if let Some(request) = self.requests.get_mut(&queued) {
                request.failure_reason = Some(INTERRUPTED.into());
            }
            closed.push(queued);
        }
        self.release_settled_interrupt_queue(agent, &closed);
        for request in &closed {
            tracing::info!(
                event = "bus.message.recovered",
                request_id = request.0,
                agent_id = agent.0,
                reason = INTERRUPTED,
                "Interrupted request closed"
            );
        }
        Ok(closed)
    }

    /// A queued request closes with the interrupted turn only when it belongs
    /// to that turn. Ordinary accepted work stays queued.
    fn queued_request_stops_with_the_turn(
        &self,
        queued: RequestId,
        current: Option<RequestId>,
        held: Option<RequestId>,
        prepared: &[RequestId],
    ) -> bool {
        let Some(request) = self.requests.get(&queued) else {
            return false;
        };
        if request.settled() || request.phase != RequestPhase::Queued {
            return false;
        }
        request.restart_continuation
            || self.is_dialog_notice(queued)
            || held.is_some_and(|id| id == queued)
            || current.is_some_and(|id| id == queued)
            || prepared.contains(&queued)
    }

    /// A request this close already settled must not stay at the head of the
    /// delivery queue. `abandon_current_request` does not remove queue ids,
    /// and the scan above skips anything that close already settled.
    fn release_settled_interrupt_queue(&mut self, agent: AgentId, closed: &[RequestId]) {
        let mut leaving = closed.to_vec();
        for id in closed {
            leaving.extend(self.group_members(*id));
        }
        leaving.retain(|id| {
            self.requests
                .get(id)
                .is_some_and(|request| request.settled())
        });
        let Some(queue) = self.queues.get_mut(&agent) else {
            return;
        };
        queue.retain(|id| !leaving.contains(id));
    }

    /// Records in `room`'s history that `author` interrupted `agent`. Like a
    /// control action, it is addressed to nobody and does not count as unread.
    pub(crate) fn post_interrupt_notice(
        &mut self,
        room: RoomId,
        author: Author,
        agent: AgentId,
        now_ms: u64,
    ) -> Result<PromptId, ModelError> {
        self.agent(agent).ok_or(ModelError::UnknownAgent(agent))?;
        let name = self.agent_message_label(agent);
        let id = PromptId(self.allocate_id());
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .notices
            .push(Prompt {
                id,
                author,
                text: format!("Interrupted {name}."),
                files: Vec::new(),
                recipient_ids: Default::default(),
                submitted_at_ms: now_ms,
                idle_agent_ids: Default::default(),
                attention_origin: None,
                in_reply_to: None,
            });
        self.cancel_pending_launch(agent, INTERRUPTED, now_ms);
        Ok(id)
    }
}

impl super::Request {
    pub(super) fn interrupt_retirement_notice(&self) -> String {
        if self.native_delivery_identity.is_none()
            || self.native_delivery_identity != self.interrupt_retirement_identity
        {
            return "Claude interrupt draft retirement is unresolved. Bus lacks the original delivery identity and cannot safely stop the original provider. The queue remains held. Please do not resend.".into();
        }
        "Claude interrupt draft retirement is unresolved. Retry the guarded interrupt or use request recover to check for the original provider exit. The queue will remain held until Bus can verify the original draft is safe. Please do not resend.".into()
    }
}
