//! Historical identities survive roster changes; queue metrics remain inspectable.
use super::{AgentId, BusState, Provider, RoomId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RetiredAgent {
    pub(super) room: RoomId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) orchestrates: Option<RoomId>,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<PathBuf>,
}

impl BusState {
    pub(crate) fn retired_agent(&self, id: AgentId) -> Option<&RetiredAgent> {
        self.retired_agents.get(&id)
    }

    /// Retired identities that still carry this exact name.
    pub(crate) fn retired_named(&self, name: &str) -> Vec<(AgentId, String)> {
        self.retired_agents
            .iter()
            .filter(|(_, agent)| agent.name == name)
            .map(|(id, agent)| (*id, agent.name.clone()))
            .collect()
    }

    /// A numeric historical identity cannot be rebound to a replacement.
    #[cfg(test)]
    pub(crate) fn is_retired_selector(&self, selector: &str) -> bool {
        selector
            .parse::<u64>()
            .ok()
            .is_some_and(|id| self.retired_agents.contains_key(&AgentId(id)))
            || self
                .retired_agents
                .values()
                .any(|agent| agent.name == selector)
    }

    pub(crate) fn agent_queue_metrics(&self, agent: AgentId, now: u64) -> (usize, Option<u64>) {
        let queue = self.queued_requests(agent);
        let oldest = queue
            .iter()
            .filter_map(|id| self.request(*id))
            .map(|r| r.prompt.submitted_at_ms)
            .min();
        (queue.len(), oldest.map(|at| now.saturating_sub(at)))
    }
}
