//! Agents operations for the messaging state.
use super::state::normalized_name;
use crate::messaging::prefs::colors;
use std::path::PathBuf;
use {
    super::{
        AgentId, AgentRuntimeIdentity, BusState, Compactions, ModelError, Provider, RoomId,
        RuntimeStatus, DEVELOPER_RECIPIENT, UNBOUND_SETTLE_MS,
    },
    crate::messaging::model::RoomAgent,
};

impl BusState {
    pub(crate) fn grant_agent_root(
        &mut self,
        id: AgentId,
        by: AgentId,
        path: PathBuf,
        now: u64,
    ) -> Result<(), String> {
        let agent = self.agent(id).ok_or("Unknown worker")?;
        if agent.permission_handler || agent.orchestrates.is_some() || agent.deletion_pending {
            return Err("Only active ordinary workers can receive roots".into());
        }
        if self
            .orchestrator_of(agent.room_id)
            .is_none_or(|a| a.id != by || a.deletion_pending)
        {
            return Err("Root grant requires this worker room's orchestrator".into());
        }
        let agent = self.agents.get_mut(&id).ok_or("Unknown worker")?;
        if !agent.allowed_roots.iter().any(|g| g.path == path) {
            agent.allowed_roots.push(super::types::AllowedRoot {
                path,
                granted_by: by,
                granted_at_ms: now,
            });
        }
        Ok(())
    }
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
            RoomAgent {
                id,
                room_id,
                name,
                color,
                accessible_color,
                permission_handler: false,
                permission_reset_required: false,
                permission_clear_started: false,
                permission_reset_at_ms: None,
                provider,
                launch_cwd: cwd.canonicalize().ok(),
                launch_cwd_backfilled: false,
                allowed_roots: Vec::new(),
                scratch_dir: None,
                launch_profile: None,
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
                update_error: None,
                current_request: None,
                hook_setup_confirmed: false,
                session_binding_invalidated: false,
                deletion_pending: false,
                orchestrates: None,
                compactions: Compactions::default(),
                last_compaction_callback: None,
                session_reset_pending: false,
                status_since_ms: 0,
                observed_at_ms: 0,
                delivery_rejection: None,
            },
        );
        self.queues.insert(id, Vec::new());
        Ok(id)
    }

    pub(crate) fn set_agent_launch_scope(
        &mut self,
        id: AgentId,
        cwd: PathBuf,
        scratch: Option<PathBuf>,
        profile: String,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.launch_cwd = Some(cwd);
        agent.scratch_dir = scratch;
        agent.launch_profile = Some(profile);
        Ok(())
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

    /// Counts the compaction hook spooled at `sequence` of `launch` once,
    /// even when its envelope is replayed after the count was saved.
    pub(crate) fn record_compaction_callback(
        &mut self,
        id: AgentId,
        launch: &str,
        sequence: u64,
        at_ms: u64,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        let key = (launch.to_owned(), sequence);
        if agent.last_compaction_callback.as_ref() == Some(&key) {
            return Ok(());
        }
        agent.last_compaction_callback = Some(key);
        self.record_compaction(id, at_ms)
    }

    pub(crate) fn rename_agent(&mut self, id: AgentId, name: &str) -> Result<(), ModelError> {
        let name = agent_name(name)?;
        if self.agent(id).is_some_and(|a| a.permission_handler) {
            return Err(ModelError::ReservedAgentName);
        }
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
        self.drop_history_of_agent(id);
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
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.session_reset_pending = true;
        agent.compactions = Compactions::default();
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

    /// Shows the update failure the agent's terminal reports as its error,
    /// and clears that error, and only that one, once none is reported.
    pub(crate) fn observe_update_error(
        &mut self,
        id: AgentId,
        error: Option<String>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        match error {
            Some(error) => {
                agent.actionable_error = Some(error.clone());
                agent.update_error = Some(error);
            }
            None => {
                if let Some(shown) = agent.update_error.take() {
                    if agent.actionable_error.as_ref() == Some(&shown) {
                        agent.actionable_error = None;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn observe_dialog(
        &mut self,
        agent: AgentId,
        dialog: bool,
    ) -> Result<(), ModelError> {
        let agent_state = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent_state.dialog = dialog;
        if dialog {
            if let Some(request) = agent_state
                .current_request
                .and_then(|id| self.requests.get_mut(&id))
            {
                request.turn_ended_at_ms = None;
            }
        }
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
            self.note_turn_end(agent, now_ms);
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
    if name.eq_ignore_ascii_case(DEVELOPER_RECIPIENT)
        || name.eq_ignore_ascii_case("permission handler")
    {
        return Err(ModelError::ReservedAgentName);
    }
    Ok(name)
}
