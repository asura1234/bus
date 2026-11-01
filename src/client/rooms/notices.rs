use super::{BusUi, Pending};
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentId, RoomId},
};
use std::collections::BTreeMap;

pub(super) struct AgentNotices {
    error: Option<String>,
    directory_error: Option<String>,
    orchestration_unavailable: bool,
    orchestration_target: Option<RoomId>,
}

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
        let mut observed = BTreeMap::new();
        for agent in self.snapshot.state.agents() {
            let current = AgentNotices {
                error: agent.actionable_error.clone(),
                directory_error: agent.working_directory_error.clone(),
                orchestration_unavailable: self.snapshot.state.orchestration_is_unavailable(agent),
                orchestration_target: agent
                    .orchestrates
                    .or(agent.unavailable_orchestration_target),
            };
            let previous = self.observed_agent_notices.get(&agent.id);
            if current.error != current.directory_error
                && previous.is_none_or(|previous| previous.error != current.error)
            {
                if let Some(error) = &current.error {
                    tracing::warn!(
                        event = "bus.agent.notice",
                        agent_id = agent.id.0,
                        agent = %agent.name,
                        room_id = agent.room_id.0,
                        %error,
                    );
                }
            }
            if previous.is_none_or(|previous| previous.directory_error != current.directory_error) {
                if let Some(error) = &current.directory_error {
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
            if current.orchestration_unavailable
                && previous.is_none_or(|previous| {
                    !previous.orchestration_unavailable
                        || previous.orchestration_target != current.orchestration_target
                })
            {
                let target = current
                    .orchestration_target
                    .map_or_else(|| "unassigned".into(), |room| room.0.to_string());
                tracing::warn!(
                    event = "bus.agent.orchestration_room_unavailable",
                    agent_id = agent.id.0,
                    agent = %agent.name,
                    room_id = agent.room_id.0,
                    target_room_id = %target,
                    error = "Orchestrated room is unavailable",
                );
            }
            observed.insert(agent.id, current);
        }
        self.observed_agent_notices = observed;
    }

    pub(super) fn sync_snapshot_error(&mut self) {
        let error = self.snapshot.error.as_deref();
        if error == self.observed_snapshot_error.as_deref() {
            return;
        }
        if let Some(error) = error {
            let mut awaiting_acknowledgement = false;
            for pending in self
                .pending
                .iter()
                .filter(|pending| pending.id <= self.snapshot.last_command_id)
            {
                match &pending.result {
                    Some(Err(cause)) if cause == error => {
                        // The contextual command record owns this projection,
                        // including coalesced snapshots of later commands.
                        self.observed_snapshot_error = Some(error.into());
                        return;
                    }
                    None => awaiting_acknowledgement = true,
                    _ => {}
                }
            }
            if awaiting_acknowledgement {
                return;
            }
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
        self.observed_snapshot_error = error.map(str::to_owned);
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
