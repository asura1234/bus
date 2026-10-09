//! Durable, code-driven worker context-limit notices.
use super::{AgentId, Author, BusState, Draft, ModelError, RoomKind};

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
        let orchestrator = self
            .orchestrator_of(room)
            .filter(|agent| !agent.deletion_pending)
            .map(|agent| agent.id);
        let to = if orchestrator.is_some() {
            "orchestrator"
        } else {
            "You"
        };
        let text = format!(
            "{} -> {to}: reached {} compactions; get a handover note and replace it.",
            agent.name, agent.compactions.count
        );
        let prompt_id = if let Some(orchestrator) = orchestrator {
            let requests = self.submit_message_from(
                room,
                Draft {
                    text,
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Agent(id),
                now_ms,
            )?;
            let request = requests
                .first()
                .and_then(|id| self.requests.get_mut(id))
                .ok_or(ModelError::InvalidTransition)?;
            request.prompt.compaction_limit_notice = true;
            request.prompt.id
        } else {
            let prompt = self.post_to_human(room, id, text, Vec::new(), now_ms)?;
            if let Some(saved) = self
                .rooms
                .get_mut(&room)
                .and_then(|room| room.notices.last_mut())
            {
                saved.compaction_limit_notice = true;
            }
            prompt
        };
        if let Some(prompt) = self
            .rooms
            .get_mut(&room)
            .and_then(|room| room.latest_prompt.as_mut())
            .filter(|prompt| prompt.id == prompt_id)
        {
            prompt.compaction_limit_notice = true;
        }
        Ok(())
    }
}
