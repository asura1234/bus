use super::{Provider, RoomAgent, RuntimeStatus, Worker, GROK_ORCHESTRATOR_RETRY};

impl Worker {
    /// Developer work for a Grok orchestrator that has no Bus turn to protect.
    ///
    /// Grok still does not steer into a live turn. This is a normal submit.
    /// The native server accepts it only when the screen is an idle composer,
    /// and refuses a dialog, a live turn, login, and a busy progress signal.
    pub(super) fn developer_message_to_grok_orchestrator(
        &self,
        agent: &RoomAgent,
        own_turn: bool,
    ) -> bool {
        if own_turn
            || agent.dialog
            || agent.current_request.is_some()
            || agent.provider != Provider::Grok
            || agent.orchestrates.is_none()
            || !matches!(
                agent.status,
                RuntimeStatus::Blocked | RuntimeStatus::Working
            )
            || self
                .grok_orchestrator_retry_at
                .get(&agent.id)
                .is_some_and(|at| at.elapsed() < GROK_ORCHESTRATOR_RETRY)
        {
            return false;
        }
        self.state
            .queued_requests(agent.id)
            .first()
            .copied()
            .is_some_and(|id| {
                self.state.request(id).is_some_and(|request| {
                    request.prompt.author == crate::messaging::model::Author::Developer
                        && self.state.can_join_turn(request)
                })
            })
    }
}
