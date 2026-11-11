//! Directory loss blocks new input; native evidence owns the current turn's end.
use super::{
    AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase, RuntimeStatus,
    TURN_SETTLE_MS,
};
use crate::messaging::model::Provider;

impl BusState {
    pub(super) fn missing_directory_turn(&self, agent: AgentId) -> Option<&Request> {
        let owner = self.agents.get(&agent)?;
        if owner.working_directory_error.is_none()
            || owner.deletion_pending
            || owner.session_binding_invalidated
        {
            return None;
        }
        self.requests
            .get(&owner.current_request?)
            .filter(|request| request.phase == RequestPhase::Active && request.trusted_start_bound)
    }

    pub(super) fn missing_directory_exit(
        &self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
    ) -> bool {
        let Some(owner) = self.agents.get(&agent) else {
            return false;
        };
        let Some(request) = self.requests.get(&id) else {
            return false;
        };
        owner.working_directory_error.is_some()
            && owner.status == RuntimeStatus::Unavailable
            && owner.exit_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.launch_id.is_some()
                    && snapshot.launch_id == request.expected_launch_id
                    && snapshot.launch_id == owner.runtime_identity.launch_id
                    && now_ms >= snapshot.at_ms.saturating_add(TURN_SETTLE_MS)
            })
    }

    pub(super) fn settle_missing_directory_turn(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let exited = self.missing_directory_exit(agent, id, now_ms);
        // Keep Kimi's live wire/retry contract; a proved exit is terminal for all providers.
        if self.agents.get(&agent)?.provider == Provider::Kimi && !exited {
            return self.settle_kimi_ended_request(agent, now_ms, recorded_end);
        }
        let request = self.requests.get(&id)?;
        if !exited && (request.awaiting_background || request.pending_final.is_some()) {
            return None;
        }
        let recorded = recorded_end(request);
        // Neither Idle nor a failed socket proves that the provider turn ended.
        if recorded.is_none() && !exited {
            return None;
        }
        self.consume_recorded_end(
            agent,
            id,
            now_ms,
            recorded,
            "rollout",
            exited,
            "Agent process exited; request closed. Inspect its terminal and resume it.",
            None,
        )
    }
}

#[cfg(test)]
#[path = "tests/missing_directory_test.rs"]
mod tests;
