//! Durable, code-driven worker context-limit notices.
use super::{AgentId, BusState, ModelError, RoomKind};

impl BusState {
    /// The notice and its latch are part of the same saved state. Raising the
    /// limit above the current count re-arms it without discarding history.
    pub(crate) fn notify_compaction_limits(
        &mut self,
        limit: u32,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        let workers: Vec<_> =
            self.agents
                .values()
                .filter(|agent| {
                    !agent.deletion_pending
                        && agent.orchestrates.is_none()
                        && self.rooms.get(&agent.room_id).is_some_and(|room| {
                            room.kind == RoomKind::Work && !room.deletion_pending
                        })
                })
                .map(|agent| (agent.id, agent.compactions))
                .collect();
        let mut changed = false;
        for (id, compactions) in workers {
            if compactions.count < limit {
                if compactions.limit_notice_sent {
                    self.agents
                        .get_mut(&id)
                        .ok_or(ModelError::UnknownAgent(id))?
                        .compactions
                        .limit_notice_sent = false;
                    changed = true;
                }
            } else if !compactions.limit_notice_sent {
                self.post_compaction_notice(id, now_ms)?;
                self.agents
                    .get_mut(&id)
                    .ok_or(ModelError::UnknownAgent(id))?
                    .compactions
                    .limit_notice_sent = true;
                changed = true;
            }
        }
        Ok(changed)
    }

    fn post_compaction_notice(&mut self, id: AgentId, now_ms: u64) -> Result<(), ModelError> {
        let agent = self.agents.get(&id).ok_or(ModelError::UnknownAgent(id))?;
        let room = agent.room_id;
        let text = format!(
            "{} reached {} compactions; get a handover note and replace it.",
            agent.name, agent.compactions.count
        );
        let orchestrator = self.notice_orchestrator(room, Some(id));
        self.post_bus_notice(room, orchestrator, text, now_ms)
            .map(|_| ())
    }
}
