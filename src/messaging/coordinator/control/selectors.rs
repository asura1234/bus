//! Shared name/ID resolution for every control command.
use super::{optional_text, required, AgentId, RoomId, Worker};
use serde_json::Value;

impl Worker {
    /// Exact IDs and unique names are global. Conversation/home rooms matter
    /// only for the reserved recipient alias.
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
        if let Some(message) = self.retired_send_target(selector, recipient_alias) {
            return Err(message);
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
        Err(format!("No agent matches {selector:?}"))
    }

    /// Live ids and names win. On a send, the orchestrator alias also wins over
    /// a retired agent that happens to be named orchestrator. Anything else
    /// that still matches a retired identity is refused before persistence.
    pub(super) fn retired_send_target(
        &self,
        selector: &str,
        recipient_alias: bool,
    ) -> Option<String> {
        if self.exact_agent(selector).is_some() {
            return None;
        }
        if let Ok(raw) = selector.parse::<u64>() {
            let id = AgentId(raw);
            if let Some(retired) = self.state.retired_agent(id) {
                return Some(crate::messaging::model::recipient_deleted_message(
                    &retired.name,
                    id,
                ));
            }
        }
        if self.state.agents().any(|agent| agent.name == selector) {
            return None;
        }
        if recipient_alias && selector.eq_ignore_ascii_case("orchestrator") {
            return None;
        }
        match self.state.retired_named(selector).as_slice() {
            [] => None,
            [(id, name)] => Some(crate::messaging::model::recipient_deleted_message(
                name, *id,
            )),
            _ => Some(format!(
                "Recipient {selector} {}",
                crate::messaging::model::RECIPIENT_DELETED_REFUSAL
            )),
        }
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
