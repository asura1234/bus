//! Native delivery text and its exact callback-matching payload.
use super::{Author, BusState, RequestId, RoomKind};

impl BusState {
    /// Freeze attribution with the rendered text so typing, retries and callback
    /// matching agree, including after a rename or a coordinator restart.
    pub(crate) fn render_request_payload(&mut self, id: RequestId) -> Option<String> {
        let request = self.requests.get(&id)?;
        if let Some(text) = &request.submitted_payload {
            return Some(text.clone());
        }
        let mut text = request.prompt.rendered_payload();
        if let Author::Agent(sender) = request.prompt.author {
            if let Some(worker) = self
                .agents
                .get(&sender)
                .filter(|a| a.room_id == request.room_id)
            {
                let to_master_orchestrator = self.agents.get(&request.agent_id).is_some_and(|a| {
                    a.orchestrates == Some(request.room_id)
                        && self
                            .rooms
                            .get(&a.room_id)
                            .is_some_and(|r| r.kind == RoomKind::Master)
                });
                if to_master_orchestrator {
                    text = format!(
                        "{} (agent {}) in room {}:\n{text}",
                        worker.name, sender.0, request.room_id.0
                    );
                }
            }
        }
        self.requests.get_mut(&id)?.submitted_payload = Some(text.clone());
        Some(text)
    }
}
