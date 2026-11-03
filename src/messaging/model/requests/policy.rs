//! Admission and grouping policy for independent orchestrator requests.
use super::{AgentId, Author, BusState, Request};

impl BusState {
    pub(super) fn agent_request_queues(
        &self,
        agent: AgentId,
        author: &Author,
        generated: bool,
    ) -> bool {
        !generated
            && matches!(author, Author::Agent(_))
            && self
                .agents
                .get(&agent)
                .is_some_and(|a| a.orchestrates.is_some())
    }

    // Older saved queues may not have the implicit queue_only flag yet.
    pub(super) fn can_join_turn(&self, request: &Request) -> bool {
        !request.queue_only
            && !self.agent_request_queues(
                request.agent_id,
                &request.prompt.author,
                request.bus_generated,
            )
    }
}
