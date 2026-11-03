//! Stable priority classes at orchestrator queue admission boundaries.
use super::{AgentId, Author, BusState, RequestId};

impl BusState {
    pub(super) fn orchestrator_queue_class(&self, id: RequestId) -> u8 {
        if self.is_dialog_notice(id) {
            0
        } else if self
            .request(id)
            .is_some_and(|request| request.prompt.author == Author::Developer)
        {
            1
        } else {
            2
        }
    }

    /// Real blocked-worker notices precede Developer messages, then ordinary
    /// work in an orchestrator's inbox. Use persisted routing and author identity,
    /// never sender-authored text.
    /// Keep FIFO order within each class. Idle delivery gives each notice its
    /// own turn; `next_steering` allows it into an already-active turn.
    pub(crate) fn prioritize_orchestrator_dialogs(&mut self, agent: AgentId) -> bool {
        if self
            .agents
            .get(&agent)
            .is_none_or(|a| a.orchestrates.is_none())
        {
            return false;
        }
        let classes: std::collections::BTreeMap<_, _> = self
            .queued_requests(agent)
            .iter()
            .copied()
            .map(|id| (id, self.orchestrator_queue_class(id)))
            .collect();
        let Some(queue) = self.queues.get_mut(&agent) else {
            return false;
        };
        let before = queue.clone();
        queue.sort_by_key(|id| classes[id]);
        let mut changed = *queue != before;
        for id in queue.iter().filter(|id| classes.get(*id) == Some(&0)) {
            if let Some(request) = self.requests.get_mut(id) {
                changed |= !request.queue_only;
                request.queue_only = true;
            }
        }
        changed
    }
}
