//! `agent interrupt`: the provider's stop key was pressed, so the running turn
//! closes instead of running on. A message that never joined that turn stays
//! queued for the next one.
use super::{
    AgentId, AgentRuntimeIdentity, Author, BusState, CallbackEventKind, ModelError, Prompt,
    PromptId, Request, RequestId, RequestPhase, RoomId, RuntimeStatus,
};
use serde::{Deserialize, Serialize};

mod replay;

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AbortedPayload {
    pub(crate) request_id: RequestId,
    pub(crate) payload: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) replay_when_returned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) cleared: bool,
}

/// Durable autonomous activity and any stop that may already have been written.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AutonomousStopState {
    pub(super) provider_turn_id: Option<String>,
    pub(super) activity: Option<(u64, u64)>,
    #[serde(default)]
    pub(super) turn_activity: Option<(u64, u64)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    task_notification: bool,
    pub(super) claim: Option<SendInterrupt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) aborted_payload: Option<AbortedPayload>,
}

impl AutonomousStopState {
    pub(super) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn task_notification_running(&self) -> bool {
        self.task_notification
    }

    fn clear_activity(&mut self) {
        self.provider_turn_id = None;
        self.activity = None;
        self.turn_activity = None;
        self.task_notification = false;
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct SendInterruptOutcome {
    pub(crate) outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

impl SendInterrupt {
    fn autonomous(owner: &super::RoomAgent) -> Self {
        let episode = &owner.autonomous_stop;
        Self {
            target: None,
            activity: episode
                .activity
                .filter(|activity| episode.turn_activity.as_ref() == Some(activity)),
            identity: owner.runtime_identity.clone(),
            submission_boundary: None,
            submission_status_revision: 0,
            provider_turn_id: episode.provider_turn_id.clone(),
        }
    }

    pub(crate) fn replayable(&self) -> bool {
        self.target.is_some()
            || (self.activity.is_some()
                && self.identity.session_id.is_some()
                && self.provider_turn_id.is_some())
    }

    fn matches_runtime(&self, identity: &AgentRuntimeIdentity) -> bool {
        self.identity.launch_id == identity.launch_id
            && self.identity.terminal_id == identity.terminal_id
            && self
                .identity
                .session_id
                .as_ref()
                .is_none_or(|session| identity.session_id.as_ref() == Some(session))
    }

    pub(crate) fn same_turn(&self, observed: &Self) -> bool {
        self.target == observed.target
            && self.submission_boundary == observed.submission_boundary
            && self.submission_status_revision == observed.submission_status_revision
            && self.matches_runtime(&observed.identity)
            && self
                .provider_turn_id
                .as_ref()
                .is_none_or(|turn| observed.provider_turn_id.as_ref() == Some(turn))
    }
}

fn is_provisional_stop_owner(request: &Request) -> bool {
    request.phase == RequestPhase::Submitting && !request.trusted_start_bound
}

impl BusState {
    pub(crate) fn send_interrupt_target(&self, recipient: AgentId) -> Option<SendInterrupt> {
        let owner = self.agent(recipient)?;
        if owner.interrupt_pending {
            return None;
        }
        let Some(target) = owner.current_request.and_then(|id| self.request(id)) else {
            return self
                .needs_interrupt_stop(owner)
                .then(|| SendInterrupt::autonomous(owner));
        };
        // A request-scoped stop on a provisional head still needs the
        // observed provider turn to distinguish consecutive autonomous episodes.
        let observed_turn = if is_provisional_stop_owner(target) {
            owner.autonomous_stop.provider_turn_id.as_ref()
        } else {
            None
        };
        Some(SendInterrupt {
            target: Some(target.id),
            activity: None,
            identity: owner.runtime_identity.clone(),
            submission_boundary: target.submission_boundary,
            submission_status_revision: target.submission_status_revision,
            provider_turn_id: observed_turn.or(target.provider_turn_id.as_ref()).cloned(),
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
        let session = owner
            .runtime_identity
            .session_id
            .as_deref()
            .filter(|session| !session.is_empty())?;
        if owner.deletion_pending
            || owner.runtime_identity.launch_id.as_deref() != Some(&callback.launch_id)
            || callback.provider_session_id.as_deref() != Some(session)
        {
            return None;
        }
        if let Some(head) = owner.current_request {
            let request = self.request(head)?;
            let payload = callback
                .prompt_payload
                .as_deref()
                .filter(|payload| !payload.trim().is_empty())?;
            // Only an unrelated Started after this reservation can advance the
            // autonomous episode alongside a still-provisional Bus owner.
            if !is_provisional_stop_owner(request)
                || !matches!(callback.kind, CallbackEventKind::PromptStarted)
                || request.expected_launch_id.as_deref() != Some(&callback.launch_id)
                || request
                    .submission_boundary
                    .is_none_or(|boundary| callback.sequence <= boundary)
                || request.matches_callback_payload(payload, owner.provider)
            {
                return None;
            }
        }
        callback
            .provider_turn_id
            .clone()
            .filter(|turn| !turn.is_empty())
    }

    pub(crate) fn observe_autonomous_turn(
        &mut self,
        id: AgentId,
        turn: String,
        started: bool,
        task_notification: bool,
    ) {
        if let Some(owner) = self.agents.get_mut(&id) {
            let activity = &mut owner.autonomous_stop;
            if started {
                // The first late Started does not prove a new episode. Two
                // distinct authenticated turns fence the claim to the earlier one.
                if let Some((previous, claim)) = activity
                    .provider_turn_id
                    .as_ref()
                    .zip(activity.claim.as_mut())
                    .filter(|(previous, claim)| {
                        *previous != &turn
                            && claim.target.is_none()
                            && claim.provider_turn_id.is_none()
                            && claim.matches_runtime(&owner.runtime_identity)
                    })
                {
                    claim.provider_turn_id = Some(previous.clone());
                }
                activity.provider_turn_id = Some(turn);
                activity.turn_activity = activity.activity;
                activity.task_notification = task_notification;
            } else if activity.provider_turn_id.as_ref() == Some(&turn) {
                activity.provider_turn_id = None;
                activity.turn_activity = None;
                activity.task_notification = false;
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
            // A crash after the claim can leave the old Claude request
            // attached even though Esc reached the pane. Reconcile that
            // request on replay while the claim prevents a second Esc.
            if intent.target.is_some_and(|id| {
                self.request(id)
                    .is_some_and(|request| !is_provisional_stop_owner(request))
            }) && self.can_replay_interrupted_turn(recipient)
            {
                self.requests
                    .get_mut(&request)
                    .expect("queued interrupt")
                    .send_interrupt = Some(intent.clone());
            }
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
        let owner = self.agent(recipient);
        let Some(id) = intent.target else {
            return owner.is_some_and(|owner| {
                let activity = &owner.autonomous_stop;
                activity.claim.as_ref().is_some_and(|claim| {
                    claim.same_turn(intent)
                        // A returned Claude prompt can outlive the stop. A
                        // transient Working frame with no new turn identity
                        // cannot authorize a second Esc for the same prompt.
                        || owner.provider.uses_claude_harness()
                            && activity.aborted_payload.is_some()
                            && intent.provider_turn_id.is_none()
                            && intent.same_turn(claim)
                })
            });
        };
        let Some(row) = self.request(id) else {
            return false;
        };
        if row
            .send_interrupt_claim
            .as_ref()
            .is_some_and(|claim| claim.same_turn(intent))
        {
            return true;
        }
        // A provisional join may still be in the autonomous episode that
        // already received a stop. A lost native receipt leaves it provisional.
        is_provisional_stop_owner(row)
            && owner
                .filter(|owner| owner.current_request == Some(id))
                .is_some_and(|owner| {
                    owner
                        .autonomous_stop
                        .claim
                        .as_ref()
                        .is_some_and(|claim| claim.same_turn(&SendInterrupt::autonomous(owner)))
                })
    }

    /// A refused join returns stop ownership to the autonomous episode.
    pub(super) fn transfer_requeued_stop_claim(&mut self, head: RequestId) {
        let Some(request) = self
            .requests
            .get_mut(&head)
            .filter(|r| !r.restart_continuation)
        else {
            return;
        };
        let Some(owner) = self
            .agents
            .get_mut(&request.agent_id)
            .filter(|a| a.current_request.is_none())
        else {
            return;
        };
        if let Some(mut claim) = request.send_interrupt_claim.take().filter(|claim| {
            claim.target == Some(head)
                && claim.provider_turn_id.is_some()
                && claim.provider_turn_id == owner.autonomous_stop.provider_turn_id
                && claim.matches_runtime(&owner.runtime_identity)
        }) {
            claim.target = None;
            claim.submission_boundary = None;
            claim.submission_status_revision = 0;
            owner.autonomous_stop.claim = Some(claim);
        }
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

    pub(crate) fn mark_interrupt_delivery(&mut self, id: RequestId) -> Result<(), ModelError> {
        self.requests
            .get_mut(&id)
            .ok_or(ModelError::UnknownRequest(id))?
            .interrupt_delivery = true;
        Ok(())
    }

    /// Screen status is a presentation observation, not proof that a bound turn
    /// stopped. An empty composer can be visible while foreground tools still run.
    /// A native final releases the current request; a background-pending Stop has
    /// already ended foreground work and needs only the ordinary request cleanup.
    pub(crate) fn needs_interrupt_stop(&self, agent: &super::RoomAgent) -> bool {
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

    /// Preserve a stop claim across immediate interrupt-mode turn closure so a
    /// back-to-back interrupt send before the next turn delivers does not press
    /// the provider's stop key a second time.
    pub(crate) fn retain_interrupt_stop_claim(&mut self, agent: AgentId) {
        let stopped_turn = self
            .agent(agent)
            .filter(|owner| owner.provider.uses_claude_harness())
            .and_then(|owner| owner.current_request)
            .and_then(|id| self.request(id))
            .and_then(|request| request.provider_turn_id.clone());
        if let Some(owner) = self.agents.get_mut(&agent) {
            if owner.autonomous_stop.claim.is_none() {
                let mut claim = SendInterrupt::autonomous(owner);
                if stopped_turn.is_some() {
                    claim.provider_turn_id = stopped_turn;
                }
                owner.autonomous_stop.claim = Some(claim);
            }
        }
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
        let aborted_id = current
            .filter(|id| closed.contains(id))
            .or_else(|| closed.last().copied());
        let aborted_payload = aborted_id.and_then(|id| {
            let payload = self.requests.get(&id)?.submitted_payload.clone()?;
            Some(AbortedPayload {
                request_id: id,
                payload,
                replay_when_returned: false,
                cleared: false,
            })
        });
        if let (Some(aborted), Some(agent_state)) = (aborted_payload, self.agents.get_mut(&agent)) {
            agent_state.autonomous_stop.aborted_payload = Some(aborted);
        }
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

    /// The native renderer proved it removed the old draft, but did not
    /// submit the replacement. Keep the old content available for a safe retry.
    pub(crate) fn retain_cleared_aborted_payload(&mut self, agent: AgentId, request: RequestId) {
        if let Some(payload) = self
            .agents
            .get_mut(&agent)
            .and_then(|owner| owner.autonomous_stop.aborted_payload.as_mut())
            .filter(|payload| payload.request_id == request)
        {
            payload.cleared = true;
        }
    }

    pub(crate) fn record_aborted_payload_discarded(
        &mut self,
        agent: AgentId,
        request_id: RequestId,
    ) {
        if let Some(agent_state) = self.agents.get_mut(&agent) {
            if agent_state
                .autonomous_stop
                .aborted_payload
                .as_ref()
                .is_some_and(|aborted| aborted.request_id == request_id)
            {
                agent_state.autonomous_stop.aborted_payload = None;
            }
        }
        if let Some(request) = self.requests.get_mut(&request_id) {
            if request.failure_reason.as_deref() == Some(INTERRUPTED) {
                request.submission_error = Some("discarded_aborted_payload".into());
            }
        }
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
                dialog_subject: None,
                in_reply_to: None,
            });
        self.cancel_pending_launch(agent, INTERRUPTED, now_ms);
        Ok(id)
    }
}
