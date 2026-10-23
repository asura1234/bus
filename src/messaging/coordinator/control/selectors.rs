//! Shared name/ID resolution for every control command.
use super::{optional_text, required, AgentId, RoomId, Worker};
use serde_json::Value;

impl Worker {
    /// Exact IDs and unique names are global. Conversation/home rooms matter
    /// only for the reserved recipient alias and removed-recipient forwarding.
    pub(super) fn resolve_agent(
        &self,
        room: Option<RoomId>,
        sender_room: Option<RoomId>,
        selector: &str,
        recipient_alias: bool,
    ) -> Result<AgentId, String> {
        if let Some(id) = self.exact_agent(selector) {
            return Ok(id);
        }
        // Normal names are globally unique. Retain the ambiguity guard for
        // legacy state read directly through the storage/recovery boundary.
        let candidates: Vec<_> = self.state.agents().filter(|a| a.name == selector).collect();
        let named = match candidates.as_slice() {
            [] => None,
            [agent] => Some(*agent),
            _ => {
                return Err(format!(
                    "Ambiguous agent name {selector:?}; use an ID: {}",
                    candidates
                        .iter()
                        .map(|a| format!("{} (agent {}, room {})", a.name, a.id.0, a.room_id.0))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        };
        // Preserve the reserved alias: a real conversation/home member wins;
        // otherwise it means the sender's bound orchestrator, never a foreign
        // agent that happens to have the reserved name.
        if recipient_alias
            && selector.eq_ignore_ascii_case("orchestrator")
            && named.is_none_or(|a| Some(a.room_id) != room && Some(a.room_id) != sender_room)
        {
            return self
                .state
                .orchestrator_of(sender_room.ok_or("This room has no bound orchestrator")?)
                .map(|a| a.id)
                .ok_or_else(|| "This room has no bound orchestrator".into());
        }
        if let Some(agent) = named {
            return Ok(agent.id);
        }
        if recipient_alias {
            if let (Some(room), Some(home)) = (room, sender_room) {
                if let Some(id) = self.state.retired_recipient(selector, room, home)? {
                    return Ok(id);
                }
            }
        }
        Err(format!("No agent matches {selector:?}"))
    }

    /// Optional context is validated for named targets; it never narrows the
    /// global namespace. An exact ID needs no author lookup.
    pub(super) fn control_agent(&self, p: &Value) -> Result<AgentId, String> {
        let room = optional_text(p, "room")?
            .map(|s| self.control_room(s))
            .transpose()?;
        let selector = required(p, "agent")?;
        if let Some(id) = self.exact_agent(selector) {
            return Ok(id);
        }
        optional_text(p, "as")
            .and_then(|value| {
                value
                    .map(|s| self.resolve_agent(room, room, s, false))
                    .transpose()
            })
            .map_err(|error| format!("--as: {error}"))?;
        self.resolve_agent(None, None, selector, false)
    }

    fn exact_agent(&self, selector: &str) -> Option<AgentId> {
        selector
            .parse::<u64>()
            .ok()
            .and_then(|id| self.state.agent(AgentId(id)))
            .map(|agent| agent.id)
    }
}
