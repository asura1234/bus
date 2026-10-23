//! Routing identities survive roster changes; queue metrics remain inspectable.
use super::{AgentId, BusState, ModelError, RoomId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct RetiredAgent {
    pub(super) room: RoomId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) orchestrates: Option<RoomId>,
    pub(super) name: String,
}

impl BusState {
    pub(crate) fn post_staffing_blocker(
        &mut self,
        agent: AgentId,
        text: String,
        now: u64,
    ) -> Result<(), ModelError> {
        let room = self
            .agent(agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .room_id;
        let recipient = self.notice_orchestrator(room, Some(agent));
        self.post_bus_notice(room, recipient, text, now).map(|_| ())
    }

    pub(crate) fn post_roster_addition(
        &mut self,
        agent: AgentId,
        now: u64,
    ) -> Result<(), ModelError> {
        let member = self.agent(agent).ok_or(ModelError::UnknownAgent(agent))?;
        let old: Vec<_> = self
            .retired_agents
            .iter()
            .filter(|(_, old)| {
                old.room == member.room_id
                    && old.name == member.name
                    && old
                        .orchestrates
                        .is_none_or(|room| member.orchestrates == Some(room))
            })
            .map(|(id, _)| id.0.to_string())
            .collect();
        let text = format!(
            "Roster change: added {} (agent {}) to room {}.{}",
            member.name,
            agent.0,
            member.room_id.0,
            if old.is_empty() {
                String::new()
            } else {
                format!(
                    " Removed IDs {} now forward to this replacement.",
                    old.join(", ")
                )
            }
        );
        self.post_bus_notice(member.room_id, None, text, now)
            .map(|_| ())
    }

    /// Only recipient resolution uses tombstones. Authors and administrative
    /// commands must always name a currently existing agent.
    pub(crate) fn retired_recipient(
        &self,
        selector: &str,
        conversation: RoomId,
        home: RoomId,
    ) -> Result<Option<AgentId>, String> {
        let retired = if let Ok(id) = selector.parse::<u64>() {
            self.retired_agents.get(&AgentId(id))
        } else {
            [conversation, home].into_iter().find_map(|room| {
                self.retired_agents.values().rev().find(|a| {
                    (a.room == room || a.orchestrates == Some(room)) && a.name == selector
                })
            })
        };
        let Some(retired) = retired else {
            return Ok(None);
        };
        let mut replacements = self.agents().filter(|a| {
            a.room_id == retired.room
                && a.name == retired.name
                && !a.deletion_pending
                && retired
                    .orchestrates
                    .is_none_or(|room| a.orchestrates == Some(room))
        });
        let first = replacements.next();
        if replacements.next().is_some() {
            return Err(format!(
                "Ambiguous replacement for {:?} in room {}; use a current ID",
                retired.name,
                retired.orchestrates.unwrap_or(retired.room).0
            ));
        }
        let recipient = first.map(|a| a.id).or_else(|| {
            self.orchestrator_of(retired.orchestrates.unwrap_or(retired.room))
                .filter(|a| !a.deletion_pending)
                .map(|a| a.id)
        });
        recipient.map(Some).ok_or_else(|| {
            format!(
                "Removed agent {:?} has no replacement or orchestrator in room {}",
                retired.name,
                retired.orchestrates.unwrap_or(retired.room).0
            )
        })
    }

    pub(crate) fn agent_queue_metrics(&self, agent: AgentId, now: u64) -> (usize, Option<u64>) {
        let queue = self.queued_requests(agent);
        let oldest = queue
            .iter()
            .filter_map(|id| self.request(*id))
            .map(|r| r.prompt.submitted_at_ms)
            .min();
        (queue.len(), oldest.map(|at| now.saturating_sub(at)))
    }
}
