//! Native delivery text and its exact callback-matching payload.
use super::{AgentId, Author, BusState, RequestId, RoomId, RoomKind};

impl BusState {
    pub(super) fn message_sender(&self, author: &Author) -> Option<(String, RoomId)> {
        let Author::Agent(id) = author else {
            return None;
        };
        self.agent(*id)
            .map(|agent| (agent.name.clone(), agent.room_id))
    }

    /// Human-facing identity; stable IDs disambiguate shared or numeric names.
    pub(crate) fn agent_label(&self, id: AgentId) -> String {
        self.agent(id).map_or_else(
            || format!("agent {}", id.0),
            |a| self.named_agent_label(id, &a.name),
        )
    }

    pub(super) fn named_agent_label(&self, id: AgentId, name: &str) -> String {
        if name.parse::<u64>().is_ok() || self.agents().any(|a| a.id != id && a.name == name) {
            format!("{name} (agent {})", id.0)
        } else {
            name.to_owned()
        }
    }

    /// Shell-safe selector for actionable commands embedded in a notice.
    pub(crate) fn agent_cli_selector(&self, id: AgentId) -> String {
        let Some(agent) = self.agent(id) else {
            return id.0.to_string();
        };
        if self.agents().any(|a| a.id != id && a.name == agent.name)
            || agent.name.parse::<u64>().is_ok()
            || agent.name.starts_with('-')
        {
            return id.0.to_string();
        }
        format!("'{}'", agent.name.replace('\'', "'\\''"))
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
                let label = self.named_agent_label(sender, &name);
                let crosses_rooms = request.room_id != room
                    || self
                        .agent(request.agent_id)
                        .is_some_and(|a| a.room_id != room)
                    || self.room(room).is_some_and(|r| r.kind == RoomKind::Master)
                    || self
                        .room(request.room_id)
                        .is_some_and(|r| r.kind == RoomKind::Master);
                let header = if crosses_rooms {
                    format!("{label} (room {}):", room.0)
                } else {
                    format!("{label}:")
                };
                if starts_with_sender(&text, &name, sender.0, room.0) {
                    text = text
                        .split_once('\n')
                        .map_or(String::new(), |(_, body)| body.to_owned());
                }
                text = if text.is_empty() {
                    header
                } else {
                    format!("{header}\n{text}")
                };
            }
        }
        self.requests.get_mut(&id)?.submitted_payload = Some(text.clone());
        Some(text)
    }
}

/// Only an exact identity-only line can be replaced. Forged IDs/rooms and
/// ordinary sentences keep their content and receive the genuine attribution.
fn starts_with_sender(text: &str, name: &str, id: u64, room: u64) -> bool {
    let first = text.lines().next().unwrap_or_default().trim();
    [
        format!("{name} (agent {id}, room {room}):"),
        format!("{name} (agent {id}) (room {room}):"),
        format!("{name} (room {room}):"),
        format!("{name} (agent {id}):"),
        format!("{name} (agent {id})."),
    ]
    .contains(&first.to_owned())
        || [name.to_owned(), format!("agent {id}")]
            .iter()
            .any(|identity| {
                [":", " here —"]
                    .iter()
                    .any(|ending| first.eq_ignore_ascii_case(&format!("{identity}{ending}")))
            })
}
