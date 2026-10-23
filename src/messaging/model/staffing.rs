//! Routing identities survive roster changes; queue metrics remain inspectable.
use super::{AgentId, BusState, Provider, RoomId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RetiredAgent {
    /// Absent only for legacy display records that never saved a home room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) room: Option<RoomId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) orchestrates: Option<RoomId>,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<PathBuf>,
}

impl BusState {
    /// The routing record is authoritative. Legacy archive records may only
    /// supply rendering metadata it lacks, never change its original binding.
    pub(crate) fn merge_retired_agent(&mut self, id: AgentId, legacy: RetiredAgent) -> bool {
        let Some(existing) = self.retired_agents.get_mut(&id) else {
            self.retired_agents.insert(id, legacy);
            return true;
        };
        let before = existing.clone();
        if existing.provider.is_none() {
            existing.provider = legacy.provider;
        }
        if existing.cwd.is_none() {
            existing.cwd = legacy.cwd;
        }
        *existing != before
    }

    pub(crate) fn retired_agent(&self, id: AgentId) -> Option<&RetiredAgent> {
        self.retired_agents.get(&id)
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
                    a.room.is_some()
                        && (a.room == Some(room) || a.orchestrates == Some(room))
                        && a.name == selector
                })
            })
        };
        let Some(retired) = retired else {
            return Ok(None);
        };
        let original_room = retired.room.ok_or_else(|| {
            format!(
                "Removed agent {:?} has no recorded original room; use a current ID",
                retired.name
            )
        })?;
        let mut replacements = self.agents().filter(|a| {
            a.room_id == original_room
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
                retired.orchestrates.unwrap_or(original_room).0
            ));
        }
        let recipient = first.map(|a| a.id).or_else(|| {
            self.orchestrator_of(retired.orchestrates.unwrap_or(original_room))
                .filter(|a| !a.deletion_pending)
                .map(|a| a.id)
        });
        recipient.map(Some).ok_or_else(|| {
            format!(
                "Removed agent {:?} has no replacement or orchestrator in room {}",
                retired.name,
                retired.orchestrates.unwrap_or(original_room).0
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
