//! Native delivery text and its exact callback-matching payload.
use super::{AgentId, Author, BusState, RequestId, RoomId, RoomKind};

impl BusState {
    pub(super) fn message_sender(&self, author: &Author) -> Option<(String, RoomId)> {
        let Author::Agent(id) = author else {
            return None;
        };
        self.agent(*id)
            .map(|agent| (agent.name.clone(), agent.room_id))
            .or_else(|| {
                self.retired_agent(*id)
                    .map(|agent| (agent.name.clone(), agent.room))
            })
    }

    /// Identity in messages; history's separate label retains its deleted marker.
    pub(crate) fn agent_message_label(&self, id: AgentId) -> String {
        self.agent(id)
            .map(|agent| agent.name.as_str())
            .or_else(|| self.retired_agent(id).map(|agent| agent.name.as_str()))
            .map_or_else(
                || format!("agent {}", id.0),
                |name| self.named_agent_label(id, name),
            )
    }

    fn named_agent_label(&self, id: AgentId, name: &str) -> String {
        if name.parse::<u64>().is_ok()
            || self.agent(id).is_none_or(|agent| agent.name != name)
            || self
                .agents()
                .any(|agent| agent.id != id && agent.name == name)
        {
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
        if agent.name.parse::<u64>().is_ok() || agent.name.starts_with('-') {
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
        let answer = format!(
            "[Bus message {}; answer with bus send --reply-to {} --as {} --text 'YOUR ANSWER']",
            request.prompt.id.0, request.prompt.id.0, request.agent_id.0,
        );
        if let Author::Agent(sender) = request.prompt.author {
            let identity = request
                .sender_identity
                .clone()
                .or_else(|| self.message_sender(&request.prompt.author));
            let (header, replace_identity) = if let Some((name, room)) = identity {
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
                (header, starts_with_sender(&text, &name, sender.0, room.0))
            } else {
                // Older saves can omit the snapshot after an author is gone.
                // Its stable ID is known; do not invent a name or home room.
                let header = format!("{}:", self.agent_message_label(sender));
                let first = text.lines().next().unwrap_or_default().trim();
                let name = format!("agent {}", sender.0);
                let replace = first.eq_ignore_ascii_case(&header)
                    || identity_only_line(first, &name, sender.0);
                (header, replace)
            };
            if replace_identity {
                text = text
                    .split_once('\n')
                    .map_or(String::new(), |(_, body)| body.to_owned());
            }
            text = if text.is_empty() {
                format!("{header}\n{answer}")
            } else {
                format!("{header}\n{answer}\n{text}")
            };
        } else {
            text = format!("{answer}\n{text}");
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
    ]
    .contains(&first.to_owned())
        || identity_only_line(first, name, id)
}

fn identity_only_line(first: &str, name: &str, id: u64) -> bool {
    [
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

/// Whether a provider's submit hook reports the prompt Bus typed as `typed`:
/// the typed text itself, or image chips with the remaining text. Claude drops
/// blank lines while lifting images; Codex's separate text paste preserves them.
pub(super) fn payload_matches(payload: &str, typed: &str) -> bool {
    use crate::agents::providers::claude_code::hooks::{
        claude_image_placeholders, claude_lifted_images,
    };
    let payload = normalized_payload(payload);
    let typed = normalized_payload(typed);
    if payload == typed {
        return true;
    }
    let (length, images) = claude_image_placeholders(&payload);
    let (lifted, remaining) = claude_lifted_images(&typed);
    let remaining = remaining.trim_end_matches(|character: char| character.is_ascii_whitespace());
    let (codex_images, codex_text) =
        crate::agents::providers::codex::hooks::image_paste_parts(&typed);
    let codex_text = normalized_payload(&codex_text);
    let actual = &payload[length..];
    images > 0
        && (actual == remaining && images == lifted
            || images == codex_images.len() && actual == codex_text.as_str())
}

fn normalized_payload(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches(|character: char| character.is_ascii_whitespace())
        .to_owned()
}
