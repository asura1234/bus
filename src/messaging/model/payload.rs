//! Native delivery text and its exact callback-matching payload.
use super::{Author, BusState, RequestId, RoomId};

impl BusState {
    pub(super) fn message_sender(&self, author: &Author) -> Option<(String, RoomId)> {
        let Author::Agent(id) = author else {
            return None;
        };
        self.agent(*id)
            .map(|agent| (agent.name.clone(), agent.room_id))
    }

    /// Freeze attribution with the rendered text so typing, retries and callback
    /// matching agree, including after a rename or a coordinator restart.
    pub(crate) fn render_request_payload(&mut self, id: RequestId) -> Option<String> {
        let request = self.requests.get(&id)?;
        if let Some(text) = &request.submitted_payload {
            return Some(text.clone());
        }
        let mut text = request.prompt.rendered_payload();
        if let Author::Agent(sender) = request.prompt.author {
            let identity = request
                .sender_identity
                .clone()
                .or_else(|| self.message_sender(&request.prompt.author));
            if let Some((name, room)) = identity {
                let header = format!("{} (agent {}, room {}):", name, sender.0, room.0);
                if !starts_with_sender(&request.prompt.text, &name, sender.0, &header) {
                    text = format!("{header}\n{text}");
                }
            }
        }
        self.requests.get_mut(&id)?.submitted_payload = Some(text.clone());
        Some(text)
    }
}

/// Only a complete identity line can replace Bus's genuine attribution.
/// Ordinary sentences and headers carrying another ID or room need the real header.
fn starts_with_sender(text: &str, name: &str, id: u64, header: &str) -> bool {
    let first = text.lines().next().unwrap_or_default().trim();
    first == header
        || [name.to_owned(), format!("agent {id}")]
            .iter()
            .any(|identity| {
                [":", " here —"]
                    .iter()
                    .any(|ending| first.eq_ignore_ascii_case(&format!("{identity}{ending}")))
            })
}
