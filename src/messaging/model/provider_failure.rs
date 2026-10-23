//! A missing provider is one recoverable room blocker, not one failed task per worker.
use super::{AgentId, BusState, ModelError, RoomId};

impl BusState {
    pub(crate) fn report_missing_binary(
        &mut self,
        room: RoomId,
        binary: &str,
        now: u64,
    ) -> Result<String, ModelError> {
        let text = format!(
            "BLOCKED: provider binary '{binary}' is missing from PATH in room {}. Tasks and saved sessions are retained. Restore '{binary}', then run: bus agent resume --room {}",
            room.0, room.0
        );
        let record = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        if record.missing_provider_binaries.insert(binary.into()) {
            let prompt = self.post_bus_notice(room, None, text.clone(), now)?;
            self.record_provider_block(room, binary, prompt, &text, now);
        }
        Ok(text)
    }

    pub(crate) fn block_missing_binary(
        &mut self,
        id: AgentId,
        binary: &str,
        now: u64,
    ) -> Result<(), ModelError> {
        let room = self.agent(id).ok_or(ModelError::UnknownAgent(id))?.room_id;
        let text = self.report_missing_binary(room, binary, now)?;
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.missing_binary = Some(binary.into());
        agent.actionable_error = Some(text);
        Ok(())
    }

    pub(crate) fn clear_missing_binary(&mut self, id: AgentId, now: u64) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if agent.missing_binary.take().is_some() {
            agent.actionable_error = None;
        }
        let (room, binary) = (agent.room_id, agent.provider.executable());
        if !self
            .agents()
            .any(|a| a.room_id == room && a.missing_binary.as_deref() == Some(binary))
        {
            if let Some(room_state) = self.rooms.get_mut(&room) {
                if room_state.missing_provider_binaries.remove(binary) {
                    self.resolve_provider_block(room, binary, now);
                }
            }
        }
        Ok(())
    }
}
