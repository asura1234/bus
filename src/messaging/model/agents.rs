//! Agents operations for the messaging state.
use crate::messaging::prefs::colors;
use std::path::PathBuf;
use {
    super::{
        AgentId, AgentRuntimeIdentity, BusState, Compactions, DialogNoticeTo, ModelError, Provider,
        RequestPhase, RoomId, RuntimeStatus, UNBOUND_SETTLE_MS,
    },
    crate::messaging::model::RoomAgent,
};

/// Why a request closed when its agent was deleted before it settled.
pub(crate) const AGENT_DELETED: &str = "agent_deleted";

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
        let name = self.available_agent_name(name, None)?;
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
                provider,
                cwd,
                branch,
                runtime_identity: AgentRuntimeIdentity::default(),
                launch_attempt_id: None,
                codex_rollout: None,
                claude_shells: None,
                startup: None,
                details_disclosed: false,
                status: RuntimeStatus::Launching,
                status_revision: 0,
                busy_revision: 0,
                dialog: false,
                dialog_notice: None,
                dialog_answered: false,
                dialog_answer: None,
                dialog_notice_to: None,
                automatic_dialog_answer: None,
                actionable_error: None,
                update_error: None,
                working_directory_error: None,
                missing_binary: None,
                current_request: None,
                provider_retry: None,
                hook_setup_confirmed: false,
                session_binding_invalidated: false,
                deletion_pending: false,
                orchestrates: None,
                unavailable_orchestration_target: None,
                compactions: Compactions::default(),
                last_compaction_callback: None,
                compaction_banner_open: false,
                session_reset_pending: false,
                superseded_session_id: None,
                interrupt_pending: false,
                status_since_ms: 0,
                observed_at_ms: 0,
                delivery_rejection: None,
                exit_snapshot: None,
                session_restart_closed_terminal: None,
            },
        );
        self.queues.insert(id, Vec::new());
        Ok(id)
    }

    pub(crate) fn record_codex_rollout(
        &mut self,
        id: AgentId,
        path: PathBuf,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .codex_rollout = Some(path);
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

    /// Counts one Grok compaction per banner episode.
    ///
    /// The banner staying visible, including a resize that redraws it, does
    /// not count again. The caller skips this when the screen was not read,
    /// so a skipped frame cannot close the latch and count the same episode
    /// twice. Closing the latch does not itself count.
    pub(crate) fn observe_compaction_banner(
        &mut self,
        id: AgentId,
        visible: bool,
        at_ms: u64,
    ) -> Result<(), ModelError> {
        {
            let agent = self
                .agents
                .get_mut(&id)
                .ok_or(ModelError::UnknownAgent(id))?;
            if !visible {
                agent.compaction_banner_open = false;
                return Ok(());
            }
            if agent.compaction_banner_open {
                return Ok(());
            }
            agent.compaction_banner_open = true;
        }
        self.record_compaction(id, at_ms)
    }

    pub(crate) fn rename_agent(&mut self, id: AgentId, name: &str) -> Result<(), ModelError> {
        if !self.agents.contains_key(&id) {
            return Err(ModelError::UnknownAgent(id));
        }
        let name = self.available_agent_name(name, Some(id))?;
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

    /// Retire the runtime; keep every message, reply and historical identity.
    pub(crate) fn delete_agent(&mut self, id: AgentId, now_ms: u64) -> Result<(), ModelError> {
        self.cancel_pending_launch(id, AGENT_DELETED, now_ms);
        let removed = self
            .agents
            .remove(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if let Some(binary) = removed.missing_binary {
            self.clear_unheld_missing_binary(removed.room_id, &binary);
        }
        self.retired_agents.insert(
            id,
            super::staffing::RetiredAgent {
                room: removed.room_id,
                orchestrates: removed.orchestrates,
                name: removed.name,
                provider: Some(removed.provider),
                cwd: Some(removed.cwd),
            },
        );
        self.queues.remove(&id);
        for request in self.requests.values_mut() {
            if request.agent_id == id && !request.settled() {
                request.phase = RequestPhase::Abandoned;
                request.completed_at_ms = Some(now_ms);
                request.failure_reason = Some(AGENT_DELETED.into());
            }
        }
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
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if agent.runtime_identity.launch_id != identity.launch_id
            || agent.runtime_identity.session_id != identity.session_id
        {
            agent.claude_shells = None;
        }
        agent.runtime_identity = identity;
        Ok(())
    }

    /// The previous server's pane is gone. Resume must not ask the replacement
    /// pane to pass the close guard that protects a still-live terminal.
    pub(crate) fn record_session_restart_closed_terminal(
        &mut self,
        id: AgentId,
        terminal_id: String,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .session_restart_closed_terminal = Some(terminal_id);
        Ok(())
    }

    pub(crate) fn clear_session_restart_closed_terminal(
        &mut self,
        id: AgentId,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .session_restart_closed_terminal = None;
        Ok(())
    }

    /// A fresh provider startup proves an empty initial boundary. Retried
    /// startup envelopes must not erase shells subsequently observed here.
    pub(crate) fn initialize_claude_shells(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent
            .claude_shells
            .get_or_insert_with(|| super::types::ClaudeShells {
                owner: agent.current_request,
                ..Default::default()
            });
        Ok(())
    }

    /// Returns whether the census contains work started since this request's
    /// known boundary. An unknown census is never proof of ownership.
    pub(crate) fn observe_claude_shells(
        &mut self,
        id: AgentId,
        running: Option<std::collections::BTreeSet<String>>,
    ) -> Result<bool, ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if agent.claude_shells.is_none() {
            let Some(running) = running else {
                return Ok(false);
            };
            agent.claude_shells = Some(super::types::ClaudeShells {
                owner: agent.current_request,
                inherited: running.clone(),
                running,
            });
            return Ok(false);
        }
        let Some(shells) = agent.claude_shells.as_mut() else {
            return Ok(false);
        };
        if shells.owner != agent.current_request {
            shells.inherited = shells.running.clone();
            shells.owner = agent.current_request;
        }
        if let Some(running) = running {
            shells.running = running;
        }
        Ok(shells
            .running
            .iter()
            .any(|task| !shells.inherited.contains(task)))
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
        agent.compaction_banner_open = false;
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
        if previous.as_deref() != Some(&session) {
            agent.claude_shells = None;
        }
        agent.superseded_session_id = previous.clone();
        agent.session_reset_pending = false;
        agent.compactions = Compactions::default();
        agent.compaction_banner_open = false;
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

    /// Keeps the screen saved for this agent's provider exit.
    pub(crate) fn record_exit_snapshot(
        &mut self,
        id: AgentId,
        snapshot: super::ExitSnapshot,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .exit_snapshot = Some(snapshot);
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

    /// Refuse future delivery without discarding an already-bound live turn.
    /// Its native record and explicit replies remain usable outside the cwd.
    pub(crate) fn fail_missing_launch_directory(
        &mut self,
        id: AgentId,
        error: String,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        let agent = self.agent(id).ok_or(ModelError::UnknownAgent(id))?;
        let previous_error = agent.actionable_error.clone();
        let mut changed = agent.working_directory_error.as_ref() != Some(&error);
        let running = agent.current_request.filter(|current| {
            self.request(*current).is_some_and(|request| {
                request.phase == RequestPhase::Active && request.trusted_start_bound
            })
        });
        let requests: Vec<_> = self
            .unsettled_requests()
            .filter(|request| {
                request.agent_id == id && Some(request.group.unwrap_or(request.id)) != running
            })
            .map(|request| request.id)
            .collect();
        for request in requests {
            if self.request(request).is_some_and(|r| !r.settled()) {
                if self
                    .request(request)
                    .is_some_and(|r| r.phase == RequestPhase::Queued)
                {
                    // Closing queued work must not clear the running turn's owner.
                    self.recover_idle_request(request, now_ms)?;
                    if let Some(request) = self.requests.get_mut(&request) {
                        request.failure_reason = Some("launch_directory_removed".into());
                    }
                } else {
                    self.close_turn_with_reason(
                        request,
                        now_ms,
                        "launch_directory_removed".into(),
                    )?;
                }
                changed = true;
            }
        }
        if let Some(queue) = self.queues.get_mut(&id) {
            queue.clear();
        }
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.current_request = running;
        if running.is_none() {
            agent.provider_retry = None;
        }
        agent.actionable_error = previous_error;
        self.observe_working_directory_error(id, Some(error))?;
        Ok(changed)
    }

    /// Directory warnings never replace another error, and recovering the
    /// directory never clears an error that arrived after this warning.
    pub(crate) fn observe_working_directory_error(
        &mut self,
        id: AgentId,
        error: Option<String>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        let previous = std::mem::replace(&mut agent.working_directory_error, error);
        if agent.actionable_error.is_none() || agent.actionable_error == previous {
            agent.actionable_error = agent.working_directory_error.clone();
        }
        if previous.is_none() && agent.working_directory_error.is_some() {
            agent.details_disclosed = true;
        }
        Ok(())
    }

    /// Closing a visible dialog starts a fresh idle interval at this observation.
    pub(crate) fn observe_dialog(
        &mut self,
        agent: AgentId,
        dialog: bool,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let agent_state = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        if agent_state.dialog && !dialog && agent_state.status == RuntimeStatus::Idle {
            agent_state.status_since_ms = now_ms;
        }
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

    pub(crate) fn set_dialog_notice_to(
        &mut self,
        agent: AgentId,
        to: Option<DialogNoticeTo>,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .dialog_notice_to = to;
        Ok(())
    }

    /// Reserve one identity-bound native UI episode before attempting input.
    pub(crate) fn reserve_automatic_dialog_answer(
        &mut self,
        agent: AgentId,
        reservation: String,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .automatic_dialog_answer = Some(reservation);
        Ok(())
    }

    pub(crate) fn clear_automatic_dialog_answer(
        &mut self,
        agent: AgentId,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .automatic_dialog_answer = None;
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
        // A provider that died or relaunched before Bus saw the stop ends
        // that interrupt (the CLI reported it failed); later work is new.
        if matches!(
            status,
            RuntimeStatus::Unavailable | RuntimeStatus::Launching
        ) {
            agent_state.interrupt_pending = false;
        }
        agent_state.observed_at_ms = now_ms;
        // Idle inference is provisional: a later busy observation of this
        // same unfinished request disproves it, even if the wall clock went
        // backwards. Captured finals and provider-owned error holds retain
        // their completion even if shells run on.
        let held_end = agent_state
            .provider_retry
            .as_ref()
            .is_some_and(|hold| agent_state.current_request == Some(hold.lead));
        if matches!(status, RuntimeStatus::Working | RuntimeStatus::Blocked) && !held_end {
            let inferred = agent_state.current_request.filter(|id| {
                self.requests.get(id).is_some_and(|request| {
                    request.turn_ended_at_ms.is_some()
                        && matches!(
                            request.phase,
                            RequestPhase::Submitting | RequestPhase::Active
                        )
                        && request.pending_final.is_none()
                })
            });
            if let Some(request) = inferred.and_then(|id| self.requests.get_mut(&id)) {
                request.turn_ended_at_ms = None;
            }
        }
        // Deletion and an invalidated session close the request their own way.
        let gone = status == RuntimeStatus::Unavailable
            && !agent_state.deletion_pending
            && !agent_state.session_binding_invalidated;
        if status == RuntimeStatus::Idle {
            // No hook reports an interrupted turn; the idle screen is its end.
            // Codex reports Idle under an approval dialog, which is not one.
            if agent_state.interrupt_pending && !agent_state.dialog {
                self.close_interrupted(agent, now_ms)?;
            }
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
        // A provider that went away cannot extend its turn, so a reply it
        // already gave completes the turn as an idle screen would. Status
        // cannot prove which turn accepted unconfirmed steering.
        if gone {
            self.complete_pending_final(agent, now_ms)?;
        }
        self.observe_bound_turn_activity(agent, status, now_ms);
        Ok(())
    }
}
