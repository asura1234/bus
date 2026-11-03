//! Reconcile an earlier independent Kimi input without replacing the newest owner.
use super::super::Provider;
use super::{AgentId, BusState, RecordedTurnEnd, RequestId};

impl BusState {
    /// A later Kimi Started can precede the earlier turn's wire flush. Settle
    /// that independent request without replacing the newest current owner.
    pub(crate) fn settle_previous_kimi_request(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded: Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        let request = self.requests.get(&id)?;
        if owner.provider != Provider::Kimi
            || request.agent_id != agent
            || request.group.is_some()
            || (!request.trusted_start_bound && !request.interrupted_by_restore)
            || request.settled()
        {
            return None;
        }
        let current = owner.current_request;
        self.agents.get_mut(&agent)?.current_request = Some(id);
        let settled = if request.interrupted_by_restore {
            self.settle_restored_turn(agent, now_ms, |_| recorded.clone())
        } else {
            self.settle_kimi_ended_request(agent, now_ms, |_| recorded)
        };
        self.agents.get_mut(&agent)?.current_request = current;
        settled
    }
}
