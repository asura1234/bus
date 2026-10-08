//! Agents operations for the messaging state.
use super::state::normalized_name;
use super::{
    Agent, AgentId, AgentRuntimeIdentity, BusState, Compactions, ModelError, Provider, RoomId,
    RuntimeStatus, HUMAN_RECIPIENT, UNBOUND_SETTLE_MS,
};
use crate::messaging::prefs::colors;
use std::path::PathBuf;

impl BusState {
    pub(crate) fn create_agent(
        &mut self,
        room_id: RoomId,
        name: &str,
        provider: Provider,
        cwd: PathBuf,
        branch: Option<String>,
    ) -> Result<AgentId, ModelError> {
        match self.rooms.get(&room_id) {
            None => return Err(ModelError::UnknownRoom(room_id)),
            Some(room) if room.deletion_pending => return Err(ModelError::DeletionPending),
            Some(_) => {}
        }
        let name = agent_name(name)?;
        let neighbors = || {
            self.agents
                .values()
                .filter(|agent| agent.room_id == room_id)
        };
        let color = colors::next_agent_color(neighbors().map(|agent| agent.color));
        let accessible_color =
            colors::next_accessible_agent_color(neighbors().map(|agent| agent.accessible_color));
        let id = AgentId(self.allocate_id());
        self.agents.insert(
            id,
            Agent {
                id,
                room_id,
                name,
                color,
                accessible_color,
                provider,
                cwd,
                branch,
                runtime_identity: AgentRuntimeIdentity::default(),
                details_disclosed: false,
                status: RuntimeStatus::Launching,
                status_revision: 0,
                busy_revision: 0,
                dialog: false,
                dialog_notice: None,
                dialog_answered: false,
                dialog_answer: None,
                actionable_error: None,
                current_request: None,
                hook_setup_confirmed: false,
                session_binding_invalidated: false,
                deletion_pending: false,
                orchestrates: None,
                compactions: Compactions::default(),
                session_reset_pending: false,
                status_since_ms: 0,
                observed_at_ms: 0,
                delivery_rejection: None,
            },
        );
        self.queues.insert(id, Vec::new());
        Ok(id)
    }

    pub(crate) fn record_compaction(&mut self, id: AgentId, at_ms: u64) -> Result<(), ModelError> {
        let compactions = &mut self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .compactions;
        compactions.count = compactions.count.saturating_add(1);
        compactions.last_at_ms = Some(at_ms);
        Ok(())
    }

    pub(crate) fn rename_agent(&mut self, id: AgentId, name: &str) -> Result<(), ModelError> {
        let name = agent_name(name)?;
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .name = name;
        Ok(())
    }

    /// Suspend delivery durably before the runtime attempts terminal shutdown.
    pub(crate) fn prepare_delete_agent(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.deletion_pending = true;
        agent.actionable_error = Some(
            "Deletion pending; queued messages are suspended. Retry deletion to finish.".into(),
        );
        Ok(())
    }

    pub(crate) fn delete_agent(&mut self, id: AgentId) -> Result<(), ModelError> {
        self.agents
            .remove(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        self.queues.remove(&id);
        self.requests.retain(|_, request| request.agent_id != id);
        for room in self.rooms.values_mut() {
            room.draft.recipient_ids.remove(&id);
            room.latest_replies.remove(&id);
        }
        Ok(())
    }

    pub(crate) fn set_agent_runtime_identity(
        &mut self,
        id: AgentId,
        identity: AgentRuntimeIdentity,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .runtime_identity = identity;
        Ok(())
    }

    pub(crate) fn confirm_hook_setup(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if agent.session_binding_invalidated {
            return Err(ModelError::InvalidTransition);
        }
        agent.hook_setup_confirmed = true;
        Ok(())
    }

    /// Marks the agent's provider context as reset by `agent clear`.
    pub(crate) fn begin_session_reset(&mut self, id: AgentId) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .session_reset_pending = true;
        Ok(())
    }

    /// Binds a reset agent to the provider session its fresh context reported.
    /// A fresh context starts with no compactions.
    pub(crate) fn rebind_reset_session(
        &mut self,
        id: AgentId,
        session: String,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        let previous = agent.runtime_identity.session_id.replace(session.clone());
        agent.session_reset_pending = false;
        agent.compactions = Compactions::default();
        // A message already sent into the fresh context was recorded under the
        // old session; its callbacks now carry the new one.
        if let Some(request) = agent
            .current_request
            .and_then(|request| self.requests.get_mut(&request))
        {
            if previous.is_some() && request.provider_session_id == previous {
                request.provider_session_id = Some(session);
            }
        }
        Ok(())
    }

    pub(crate) fn invalidate_agent_session(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.session_binding_invalidated = true;
        agent.hook_setup_confirmed = false;
        Ok(())
    }

    pub(crate) fn set_agent_details_disclosed(
        &mut self,
        id: AgentId,
        disclosed: bool,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .details_disclosed = disclosed;
        Ok(())
    }

    pub(crate) fn update_agent_runtime_metadata(
        &mut self,
        id: AgentId,
        cwd: PathBuf,
        branch: Option<String>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.cwd = cwd;
        agent.branch = branch;
        Ok(())
    }

    pub(crate) fn set_agent_error(
        &mut self,
        id: AgentId,
        error: Option<String>,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .actionable_error = error;
        Ok(())
    }

    pub(crate) fn observe_dialog(
        &mut self,
        agent: AgentId,
        dialog: bool,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .dialog = dialog;
        Ok(())
    }

    /// Records what Bus reported the agent waiting on, or `None` once it cleared.
    pub(crate) fn set_dialog_notice(
        &mut self,
        agent: AgentId,
        notice: Option<String>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent.dialog_notice = notice;
        agent.dialog_answered = false;
        agent.dialog_answer = None;
        Ok(())
    }

    pub(crate) fn mark_dialog_answered(
        &mut self,
        agent: AgentId,
        option: u32,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent.dialog_answered = true;
        agent.dialog_answer = Some(option);
        Ok(())
    }

    pub(crate) fn observe_status(
        &mut self,
        agent: AgentId,
        status: RuntimeStatus,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let revision = self.next_status_revision;
        self.next_status_revision = self.next_status_revision.saturating_add(1);
        let agent_state = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        if agent_state.status != status {
            agent_state.status_since_ms = now_ms;
        }
        agent_state.status = status;
        agent_state.status_revision = revision;
        if matches!(status, RuntimeStatus::Working | RuntimeStatus::Blocked) {
            agent_state.busy_revision = revision;
        }
        agent_state.observed_at_ms = now_ms;
        if status == RuntimeStatus::Idle {
            self.complete_pending_final(agent, now_ms)?;
            let settled_idle = self.unbound_request(agent).is_some_and(|request| {
                request
                    .foreign_turn_settled_at_ms
                    .is_some_and(|at| now_ms >= at.saturating_add(UNBOUND_SETTLE_MS))
            });
            if settled_idle {
                self.release_unbound_request(agent, now_ms);
            }
        }
        Ok(())
    }
}

fn agent_name(name: &str) -> Result<String, ModelError> {
    let name = normalized_name(name)?;
    if name.eq_ignore_ascii_case(HUMAN_RECIPIENT) {
        return Err(ModelError::ReservedAgentName);
    }
    Ok(name)
}
