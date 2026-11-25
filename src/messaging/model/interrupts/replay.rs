//! Requeue unfinished Claude work ahead of its interrupting send.
use super::{
    AbortedPayload, AgentId, BusState, ModelError, RequestId, RuntimeStatus, SendInterrupt,
};

impl BusState {
    pub(crate) fn can_replay_interrupted_turn(&self, agent: AgentId) -> bool {
        self.agents.get(&agent).is_some_and(|owner| {
            owner.provider.uses_claude_harness()
                && owner.provider_retry.is_none()
                && owner
                    .current_request
                    .and_then(|id| self.request(id))
                    .is_some_and(|request| {
                        !request.restart_continuation && request.pending_final.is_none()
                    })
        })
    }

    /// Keep the original request IDs so both implicit and explicit replies can
    /// settle the replayed work. Earlier queued work keeps its lane in the queue.
    pub(crate) fn requeue_interrupted_turn(
        &mut self,
        agent: AgentId,
        replacement: RequestId,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let lead = self
            .agent(agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .current_request
            .ok_or(ModelError::InvalidTransition)?;
        let pending: Vec<_> = std::iter::once(lead)
            .chain(self.group_members(lead))
            .filter(|id| self.request(*id).is_some_and(|request| !request.settled()))
            .collect();
        let payload = self.requests[&lead].submitted_payload.clone();
        // Even an explicitly answered request can still anchor a native turn.
        // Its old callbacks must not attach to the replacement.
        if self.requests[&lead].settled() {
            self.retire_provider_turn(lead);
        }
        for id in &pending {
            let row = self.reset_request_for_replay(*id, now_ms);
            row.interrupt_replay_before = Some(replacement);
            row.interrupt_delivery = false;
            row.bound_turn_activity = None;
            row.delivery_epoch = None;
            row.send_interrupt_claim = None;
        }
        let queue = self
            .queues
            .get_mut(&agent)
            .ok_or(ModelError::InvalidTransition)?;
        let index = queue
            .iter()
            .position(|id| *id == replacement)
            .ok_or(ModelError::InvalidTransition)?;
        queue.splice(index..index, pending);
        let owner = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        owner.current_request = None;
        owner.interrupt_pending = false;
        if let Some(payload) = payload {
            owner.autonomous_stop.aborted_payload = Some(AbortedPayload {
                request_id: lead,
                payload,
            });
        }
        Ok(())
    }

    pub(in crate::messaging::model) fn interrupted_batch(
        &self,
        following: &[RequestId],
        replacement: RequestId,
    ) -> Vec<RequestId> {
        // The stopped requests are adjacent to the replacing send, even
        // when their original delivery lanes or conversation rooms differ.
        let mut replay: Vec<_> = following
            .iter()
            .copied()
            .take_while(|id| self.requests[id].interrupt_replay_before == Some(replacement))
            .collect();
        if following.get(replay.len()) == Some(&replacement) {
            replay.push(replacement);
        }
        replay
    }

    pub(in crate::messaging::model) fn finish_interrupt_replay(&mut self, lead: RequestId) {
        if self
            .requests
            .get_mut(&lead)
            .and_then(|row| row.interrupt_replay_before.take())
            .is_some()
        {
            for id in self.group_members(lead) {
                self.requests
                    .get_mut(&id)
                    .expect("replayed member")
                    .interrupt_replay_before = None;
            }
        }
    }

    pub(crate) fn claude_autonomous_interrupt_target(
        &self,
        recipient: AgentId,
    ) -> Option<SendInterrupt> {
        let owner = self.agent(recipient)?;
        if !owner.provider.uses_claude_harness()
            || owner.current_request.is_some()
            || owner.status != RuntimeStatus::Working
            || !owner.autonomous_stop.task_notification_running()
            || owner.dialog
            || owner.deletion_pending
            || owner.session_binding_invalidated
        {
            return None;
        }
        let target = self.send_interrupt_target(recipient)?;
        (target.provider_turn_id.is_some()
            && !self.send_interrupt_already_claimed(recipient, &target))
        .then_some(target)
    }
}
