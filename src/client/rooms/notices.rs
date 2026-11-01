use super::{BusUi, Pending};
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentId, RoomId},
};

impl BusUi {
    pub(super) fn record_command_failure(
        &mut self,
        pending: &Pending,
        error: &str,
        recovered: bool,
        keep: bool,
    ) {
        let (agent, field, path) = match &pending.command {
            BusCommand::AddAgent(input) | BusCommand::AddOrchestrator(input, _) => (
                Some(input.name.as_str()),
                error.contains("PWD").then_some("pwd"),
                Some(input.cwd.clone()),
            ),
            BusCommand::AttachFile(_, path) => (None, Some("file"), Some(path.clone())),
            BusCommand::RemoveFile(_, path) => {
                (None, Some("file"), Some(path.display().to_string()))
            }
            BusCommand::Suggestions { input, .. } => (None, Some("path"), Some(input.clone())),
            _ => (None, None, None),
        };
        tracing::warn!(
            event = "bus.command.failed",
            command_id = pending.id,
            command = crate::messaging::perf::command_name(&pending.command),
            room_id = ?command_room(&pending.command).map(|room| room.0),
            agent_id = ?command_agent(&pending.command).map(|agent| agent.0),
            agent = agent.unwrap_or(""),
            field = field.unwrap_or(""),
            path = path.as_deref().unwrap_or(""),
            recovered,
            keep,
            %error,
        );
    }

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

pub(super) fn command_room(command: &BusCommand) -> Option<RoomId> {
    match command {
        BusCommand::RenameRoom(room, _)
        | BusCommand::SetRoomSound(room, _)
        | BusCommand::SetRoomSoundName(room, _)
        | BusCommand::DeleteRoom(room)
        | BusCommand::SelectRoom(room)
        | BusCommand::MarkRoomSeen(room)
        | BusCommand::SetNotes(room, _)
        | BusCommand::SetDraftText(room, _)
        | BusCommand::SetRecipients(room, _)
        | BusCommand::AttachFile(room, _)
        | BusCommand::RemoveFile(room, _)
        | BusCommand::Submit(room)
        | BusCommand::SubmitQueued(room) => Some(*room),
        BusCommand::AddAgent(input) | BusCommand::AddOrchestrator(input, _) => Some(input.room),
        _ => None,
    }
}

pub(super) fn command_agent(command: &BusCommand) -> Option<AgentId> {
    match command {
        BusCommand::RenameAgent(agent, _)
        | BusCommand::DeleteAgent(agent)
        | BusCommand::SetDetails(agent, _)
        | BusCommand::FocusTerminal(agent)
        | BusCommand::CompleteHookSetup(agent) => Some(*agent),
        _ => None,
    }
}
