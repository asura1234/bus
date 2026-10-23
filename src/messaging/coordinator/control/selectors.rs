//! Shared name/ID resolution for every control command.
use super::{optional_text, required, AgentId, RoomId, Worker};
use serde_json::Value;

impl Worker {
    /// Exact IDs are global; names prefer the conversation, then sender's room. The reserved
    /// alias belongs only to `--to`, after real local names have had precedence.
    pub(super) fn resolve_agent(
        &self,
        room: Option<RoomId>,
        sender_room: Option<RoomId>,
        selector: &str,
        recipient_alias: bool,
    ) -> Result<AgentId, String> {
        if let Some(agent) = selector
            .parse::<u64>()
            .ok()
            .and_then(|id| self.state.agent(AgentId(id)))
        {
            return Ok(agent.id);
        }
        let mut candidates: Vec<_> = self.state.agents().filter(|a| a.name == selector).collect();
        if let Some(scope) = [room, sender_room]
            .into_iter()
            .flatten()
            .find(|scope| candidates.iter().any(|a| a.room_id == *scope))
        {
            candidates.retain(|a| a.room_id == scope);
        } else if recipient_alias && selector.eq_ignore_ascii_case("orchestrator") {
            return self
                .state
                .orchestrator_of(sender_room.ok_or("This room has no bound orchestrator")?)
                .map(|a| a.id)
                .ok_or_else(|| "This room has no bound orchestrator".into());
        }
        if candidates.is_empty() && recipient_alias {
            if let (Some(room), Some(home)) = (room, sender_room) {
                if let Some(id) = self.state.retired_recipient(selector, room, home)? {
                    return Ok(id);
                }
            }
        }
        match candidates.as_slice() {
            [agent] => Ok(agent.id),
            [] => Err("No matching room or agent".into()),
            _ => Err(format!(
                "Ambiguous agent name {selector:?}; use an ID: {}",
                candidates
                    .iter()
                    .map(|a| format!("{} (agent {}, room {})", a.name, a.id.0, a.room_id.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Agent commands have optional conversation and sender context. Without
    /// either, names are global; exact IDs remain global in every context.
    pub(super) fn scoped_agent(&self, p: &Value) -> Result<AgentId, String> {
        let room = optional_text(p, "room")?
            .map(|s| self.control_room(s))
            .transpose()?;
        let author = optional_text(p, "as")?
            .map(|s| self.resolve_agent(room, room, s, false))
            .transpose()?;
        let home = author
            .and_then(|id| self.state.agent(id))
            .map(|a| a.room_id)
            .or(room);
        self.resolve_agent(room, home, required(p, "agent")?, false)
    }
}
