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
                if !starts_with_sender(&request.prompt.text, &name, sender.0) {
                    text = format!("{} (agent {}, room {}):\n{text}", name, sender.0, room.0);
                }
            }
        }
        self.requests.get_mut(&id)?.submitted_payload = Some(text.clone());
        Some(text)
    }
}

/// Recognize a leading identity line, including older hand-written headers.
/// A name or ID embedded later in the message does not identify its author.
fn starts_with_sender(text: &str, name: &str, id: u64) -> bool {
    let first = text.trim_start().lines().next().unwrap_or_default().trim();
    let first = first
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case("from "))
        .map_or(first, |_| first[5..].trim_start());
    identity_prefix(first, name) || identity_prefix(first, &format!("agent {id}"))
}

fn identity_prefix(line: &str, identity: &str) -> bool {
    !identity.is_empty()
        && line
            .get(..identity.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(identity))
        && line.get(identity.len()..).is_some_and(|rest| {
            rest.chars().next().is_none_or(|c| {
                c.is_whitespace() || matches!(c, '(' | ')' | ':' | ',' | '.' | '—' | '–' | '-')
            })
        })
}
