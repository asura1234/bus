use super::BusUi;

impl BusUi {
    pub(super) fn sync_agent_notices(&mut self) {
        let agents: Vec<_> = self.snapshot.state.agents().collect();
        for agent in &agents {
            let previous = self.observed_agent_notices.get(&agent.id);
            if previous.is_none_or(|(error, _, _)| error != &agent.actionable_error) {
                if let Some(error) = &agent.actionable_error {
                    tracing::warn!(
                        event = "bus.agent.notice",
                        agent_id = agent.id.0,
                        agent = %agent.name,
                        room_id = agent.room_id.0,
                        %error,
                    );
                }
            }
            if previous.is_none_or(|(_, error, _)| error != &agent.working_directory_error) {
                if let Some(error) = &agent.working_directory_error {
                    tracing::warn!(
                        event = "bus.agent.working_directory_failed",
                        agent_id = agent.id.0,
                        agent = %agent.name,
                        room_id = agent.room_id.0,
                        path = %agent.cwd.display(),
                        %error,
                    );
                }
            }
            let missing_room = agent
                .orchestrates
                .filter(|room| self.snapshot.state.room(*room).is_none());
            if previous.is_none_or(|(_, _, previous)| previous != &missing_room) {
                if let Some(target) = missing_room {
                    tracing::warn!(
                        event = "bus.agent.orchestration_room_unavailable",
                        agent_id = agent.id.0,
                        agent = %agent.name,
                        room_id = agent.room_id.0,
                        target_room_id = target.0,
                        error = "Orchestrated room is unavailable",
                    );
                }
            }
        }
        self.observed_agent_notices = agents
            .into_iter()
            .map(|agent| {
                let missing_room = agent
                    .orchestrates
                    .filter(|room| self.snapshot.state.room(*room).is_none());
                (
                    agent.id,
                    (
                        agent.actionable_error.clone(),
                        agent.working_directory_error.clone(),
                        missing_room,
                    ),
                )
            })
            .collect();
    }

    pub(super) fn sync_snapshot_error(&self, previous: Option<&str>) {
        let error = self.snapshot.error.as_deref();
        if error == previous {
            return;
        }
        if let Some(error) = error {
            let event = if super::storage_paused_error(error) {
                "bus.storage.paused"
            } else {
                "bus.coordinator.error"
            };
            tracing::warn!(
                event,
                last_command_id = self.snapshot.last_command_id,
                %error,
            );
        }
    }
}
