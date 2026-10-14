//! `agent interrupt`: the provider's stop key was pressed, so the running turn
//! and the messages waiting for the agent close instead of running on.
use super::{AgentId, Author, BusState, ModelError, Prompt, PromptId, RequestId, RoomId};

/// The reason an interrupted request closed with, which `wait` reports as
/// `message_interrupted`.
pub(crate) const INTERRUPTED: &str = "interrupted";

impl BusState {
    /// Records that the stop key went to `agent`'s terminal. The turn closes
    /// when Bus next sees the agent idle, not before: the provider decides.
    pub(crate) fn begin_interrupt(&mut self, agent: AgentId) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .interrupt_pending = true;
        Ok(())
    }

    /// Closes `agent`'s open turn, with every message typed into it, and its
    /// queue as interrupted. A reply captured before the stop took effect means
    /// the turn ended on its own first: that reply still completes it.
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
        let current = agent_state.current_request;
        let mut closed = Vec::new();
        if let Some(current) = current.filter(|id| {
            self.requests
                .get(id)
                .is_some_and(|request| request.pending_final.is_none())
        }) {
            self.close_turn_with_reason(current, now_ms, INTERRUPTED.into())?;
            closed.push(current);
        }
        for queued in self.queued_requests(agent).to_vec() {
            self.recover_idle_request(queued, now_ms)?;
            if let Some(request) = self.requests.get_mut(&queued) {
                request.failure_reason = Some(INTERRUPTED.into());
            }
            closed.push(queued);
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

    /// Records in `room`'s history that `author` interrupted `agent`. Like a
    /// control action, it is addressed to nobody and does not count as unread.
    pub(crate) fn post_interrupt_notice(
        &mut self,
        room: RoomId,
        author: Author,
        agent: AgentId,
        now_ms: u64,
    ) -> Result<PromptId, ModelError> {
        let name = self
            .agents
            .get(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .name
            .clone();
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
                compaction_limit_notice: false,
            });
        Ok(id)
    }
}
