//! Optimistic room entries for Add submissions, reconciled by command ID.
use super::{forms::Form, BusUi, Effect};
use crate::{
    agents::providers::launch::SetupNotice,
    messaging::{
        coordinator::{agents::AddAgent, BusCommand},
        model::{AgentId, LaunchKind, LaunchOutcome, RoomId},
        orchestration::OrchestratorSpec,
    },
};

pub(super) struct AgentCreation {
    pub(super) room: RoomId,
    pub(super) name: String,
    pub(super) state: CreationState,
    pub(super) roster_agent: Option<AgentId>,
    form_generation: u64,
}

pub(super) enum CreationState {
    Launching,
    NeedsConsent(Box<Form>),
    Failed(String),
}

impl BusUi {
    /// Only the snapshot's roster entries can receive messages or open terminals.
    pub(super) fn provisional_agents(&self) -> impl Iterator<Item = (&u64, &AgentCreation)> {
        self.agent_creations.iter().filter(|(_, creation)| {
            Some(creation.room) == self.room
                && creation.roster_agent.is_none()
                && !self
                    .snapshot
                    .state
                    .agents()
                    .any(|agent| agent.room_id == creation.room && agent.name == creation.name)
        })
    }

    pub(super) fn agent_creation_errors(&self, room: RoomId) -> impl Iterator<Item = (&str, &str)> {
        let refused =
            self.agent_creations
                .values()
                .rev()
                .filter_map(move |creation| match &creation.state {
                    CreationState::Failed(error) if creation.room == room => {
                        Some((creation.name.as_str(), error.as_str()))
                    }
                    _ => None,
                });
        let state = &self.snapshot.state;
        let failed_startups = state.agents().filter_map(move |agent| {
            if agent.room_id != room
                || self
                    .agent_creations
                    .values()
                    .any(|creation| creation.roster_agent == Some(agent.id))
            {
                return None;
            }
            let launch = state.current_launch_attempt(agent.id)?;
            (launch.kind == LaunchKind::Create && launch.outcome == LaunchOutcome::Failed)
                .then_some((agent.name.as_str(), agent.actionable_error.as_deref()?))
        });
        refused.chain(failed_startups)
    }

    pub(super) fn validate_agent_form_name(&self, name: &str) -> Result<String, String> {
        let name = self
            .snapshot
            .state
            .available_agent_name(name, None)
            .map_err(|error| error.to_string())?;
        if self.agent_creations.values().any(|creation| {
            creation.name == name && !matches!(creation.state, CreationState::Failed(_))
        }) {
            return Err(format!("Agent name {name:?} already has a pending launch."));
        }
        Ok(name)
    }

    pub(super) fn submit_agent_creation(
        &mut self,
        command: BusCommand,
        room: RoomId,
        name: String,
    ) {
        let id = self.queue(command, Effect::None);
        self.open_room(room);
        self.agent_creations.retain(|_, creation| {
            creation.room != room
                || creation.name != name
                || !matches!(creation.state, CreationState::Failed(_))
        });
        self.agent_creations.insert(
            id,
            AgentCreation {
                room,
                name,
                state: CreationState::Launching,
                roster_agent: None,
                form_generation: self.form_generation,
            },
        );
    }

    pub(super) fn receive_agent_setup(
        &mut self,
        command_id: u64,
        input: AddAgent,
        orchestrator: Option<OrchestratorSpec>,
        notice: SetupNotice,
    ) {
        let Some(creation) = self.agent_creations.get_mut(&command_id) else {
            return;
        };
        creation.state = CreationState::NeedsConsent(Box::new(Form::Consent {
            input,
            orchestrator,
            notice,
        }));
        if self.room == Some(creation.room)
            && self.form.is_none()
            && self.rename.is_none()
            && self.terminal.is_none()
            && self.form_generation == creation.form_generation
        {
            self.review_agent_setup(command_id);
        }
    }

    pub(super) fn review_agent_setup(&mut self, command_id: u64) {
        let Some(CreationState::NeedsConsent(_)) = self
            .agent_creations
            .get(&command_id)
            .map(|creation| &creation.state)
        else {
            return;
        };
        if let Some(AgentCreation {
            state: CreationState::NeedsConsent(form),
            ..
        }) = self.agent_creations.remove(&command_id)
        {
            self.open_form(*form);
        }
    }

    pub(super) fn settle_agent_creation(&mut self, command_id: u64, result: &Result<(), String>) {
        let Some(creation) = self.agent_creations.get_mut(&command_id) else {
            return;
        };
        match result {
            Ok(()) if matches!(creation.state, CreationState::Launching) => {
                self.agent_creations.remove(&command_id);
            }
            Ok(()) => {}
            Err(error) => {
                // A native refusal can arrive after Bus saved the roster entry.
                // Bind its notice to that exact agent so rename or deletion
                // cannot turn it back into a provisional row.
                creation.roster_agent = self.snapshot.state.agents().find_map(|agent| {
                    (agent.room_id == creation.room
                        && agent.name == creation.name
                        && agent.actionable_error.as_deref() == Some(error.as_str()))
                    .then_some(agent.id)
                });
                creation.state = CreationState::Failed(error.clone());
            }
        }
    }
}
