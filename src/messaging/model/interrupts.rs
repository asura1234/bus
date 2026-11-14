//! `agent interrupt`: the provider's stop key was pressed, so the running turn
//! closes instead of running on. A message that never joined that turn stays
//! queued for the next one.
use super::{
    AgentId, Author, BusState, ModelError, Prompt, PromptId, RequestId, RequestPhase, RoomId,
};

/// The reason an interrupted request closed with, which `wait` reports as
/// `message_interrupted`.
pub(crate) const INTERRUPTED: &str = "interrupted";

impl BusState {
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
