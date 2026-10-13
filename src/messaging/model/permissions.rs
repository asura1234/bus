//! Durable permission leases. Bus routes every blocked screen; Claude classifies it.
use super::{
    AgentId, Author, BusState, Draft, ModelError, Provider, RoomId, RoomKind, RuntimeStatus,
};
use serde::{Deserialize, Serialize};

pub(crate) const POLICY_VERSION: &str = "permission-handler-v1";
pub(crate) const DEADLINE_MS: u64 = 90_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PermissionOutcome {
    Approved,
    Escalated,
    Routed,
    Fallback,
    Superseded,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PermissionAssignment {
    pub(crate) token: String,
    pub(crate) room: RoomId,
    pub(crate) worker: AgentId,
    pub(crate) worker_launch: Option<String>,
    pub(crate) handler: Option<AgentId>,
    pub(crate) generation: Option<String>,
    pub(crate) episode: String,
    pub(crate) dispatched_at_ms: u64,
    pub(crate) deadline_ms: u64,
    pub(crate) delivered: bool,
    #[serde(default)]
    pub(crate) request: Option<super::RequestId>,
    pub(crate) answer_attempted: bool,
    pub(crate) answer_confirmed: bool,
    pub(crate) fingerprint: Option<String>,
    pub(crate) outcome: Option<PermissionOutcome>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PermissionDecision {
    pub(crate) assignment: PermissionAssignment,
    pub(crate) at_ms: u64,
    pub(crate) policy_version: String,
    pub(crate) summary: String,
}
impl BusState {
    pub(crate) fn permission_reset_started(
        &mut self,
        id: AgentId,
        now: u64,
    ) -> Result<(), ModelError> {
        let a = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        a.permission_clear_started = true;
        a.permission_reset_at_ms = Some(now);
        Ok(())
    }
    pub(crate) fn finish_permission_reset(&mut self, id: AgentId) -> Result<(), ModelError> {
        let a = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        a.permission_reset_required = false;
        a.permission_clear_started = false;
        a.permission_reset_at_ms = None;
        Ok(())
    }
    pub(crate) fn permission_assignments(&self) -> impl Iterator<Item = &PermissionAssignment> {
        self.permission_assignments.values()
    }
    pub(crate) fn prune_permission_assignments(&mut self) {
        let agents = &self.agents;
        let rooms = &self.rooms;
        self.permission_assignments.retain(|worker, p| {
            p.outcome.is_none() || (agents.contains_key(worker) && rooms.contains_key(&p.room))
        });
    }
    pub(crate) fn permission_decisions(&self) -> &[PermissionDecision] {
        &self.permission_decisions
    }
    pub(crate) fn permission_assignment(&self, worker: AgentId) -> Option<&PermissionAssignment> {
        self.permission_assignments.get(&worker)
    }
    pub(crate) fn permission_handler_of(&self, room: RoomId) -> Option<&super::RoomAgent> {
        self.agents()
            .find(|a| a.room_id == room && a.permission_handler)
    }
    pub(crate) fn mark_permission_handler(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self.agents.get(&id).ok_or(ModelError::UnknownAgent(id))?;
        if agent.provider != Provider::ClaudeCode
            || self
                .room(agent.room_id)
                .is_none_or(|r| r.kind != RoomKind::Work)
            || self.permission_handler_of(agent.room_id).is_some()
        {
            return Err(ModelError::InvalidTransition);
        }
        let agent = self.agents.get_mut(&id).unwrap();
        agent.permission_handler = true;
        agent.name = "permission handler".into();
        Ok(())
    }
    pub(crate) fn permission_handler_ready(&self, id: AgentId) -> bool {
        self.agent(id).is_some_and(|a| {
            a.permission_handler
                && !a.deletion_pending
                && !a.session_binding_invalidated
                && a.hook_setup_confirmed
                && a.runtime_identity.session_id.is_some()
                && a.runtime_identity.launch_id.is_some()
                && a.actionable_error.is_none()
                && a.compactions.count == 0
                && !a.dialog
                && a.status != RuntimeStatus::Blocked
                && matches!(a.status, RuntimeStatus::Idle | RuntimeStatus::Working)
        })
    }
    /// No interpretation of the dialog enters this operation.
    pub(crate) fn dispatch_permission(
        &mut self,
        worker: AgentId,
        episode: String,
        now: u64,
    ) -> Result<(), String> {
        let a = self.agent(worker).ok_or("Unknown agent")?.clone();
        let room = a.orchestrates.unwrap_or(a.room_id);
        if self
            .room(room)
            .is_none_or(|r| r.kind != RoomKind::Work || r.deletion_pending)
        {
            return Ok(());
        }
        if self
            .permission_assignment(worker)
            .is_some_and(|p| p.outcome.is_none())
        {
            self.complete_permission(
                worker,
                PermissionOutcome::Superseded,
                "new dialog episode",
                now,
            )?;
        }
        let handler = self.permission_handler_of(room).map(|a| a.id);
        let generation = handler.and_then(|id| self.agent(id)?.runtime_identity.launch_id.clone());
        let token = format!("p{}-{}", self.allocate_id(), now);
        self.permission_assignments.insert(
            worker,
            PermissionAssignment {
                token,
                room,
                worker,
                worker_launch: a.runtime_identity.launch_id.clone(),
                handler,
                generation,
                episode,
                dispatched_at_ms: now,
                deadline_ms: now.saturating_add(DEADLINE_MS),
                delivered: false,
                request: None,
                answer_attempted: false,
                answer_confirmed: false,
                fingerprint: None,
                outcome: None,
            },
        );
        if a.permission_handler || handler.is_none_or(|id| !self.permission_handler_ready(id)) {
            return self.complete_permission(
                worker,
                PermissionOutcome::Fallback,
                "permission handler unavailable; Blocked, needs help to continue.",
                now,
            );
        }
        self.deliver_permission(worker, now)
    }
    pub(crate) fn deliver_permission(&mut self, worker: AgentId, now: u64) -> Result<(), String> {
        let p = self
            .permission_assignment(worker)
            .ok_or("Unknown assignment")?
            .clone();
        let Some(handler) = p.handler else {
            return Ok(());
        };
        if p.outcome.is_some()
            || p.delivered
            || self
                .agent(handler)
                .is_none_or(|a| a.permission_reset_required || a.session_reset_pending)
            || self.permission_assignments().any(|other| {
                other.worker != worker
                    && other.handler == Some(handler)
                    && other.delivered
                    && other.outcome.is_none()
            })
        {
            return Ok(());
        }
        let agent = self.agent(worker).ok_or("Unknown worker")?;
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let cli = crate::platform::remote_reattach_program(&executable.to_string_lossy());
        let root = agent
            .launch_cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown (legacy); escalate".into());
        let scratch = agent
            .scratch_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none assigned".into());
        let text = format!("Permission assignment {}. Worker {} (agent {}), handler {}. Inspect: {cli} agent dialog {}. Frozen authorized worker root: {}. Actual observed cwd: {}. Assigned private scratch: {}. Read fresh room workflow authorization with {cli} state and the room's workflow files; the listed safe classes have standing user authorization; other unclear authority means escalate. Deadline {}. Complete with {cli} agent permission-decision {} --handler {} --assignment {} --fingerprint FINGERPRINT_OR_unavailable --outcome approved|escalated|routed --summary ONE_LINE. Managed choose/answer must include --handler {} --assignment {}. Never answer an unknown screen.", p.token, agent.name, worker.0, handler.0, worker.0, root, agent.cwd.display(), scratch, p.deadline_ms, worker.0, handler.0, p.token, handler.0, p.token);
        let requests = self
            .submit_message_with(
                p.room,
                Draft {
                    text,
                    files: vec![],
                    recipient_ids: [handler].into(),
                },
                Author::Agent(worker),
                now,
                true,
            )
            .map_err(|e| e.to_string())?;
        let assignment = self.permission_assignments.get_mut(&worker).unwrap();
        assignment.delivered = true;
        assignment.request = requests.first().copied();
        Ok(())
    }
    pub(crate) fn validate_permission_lease(
        &self,
        worker: AgentId,
        handler: AgentId,
        token: &str,
        now: u64,
    ) -> Result<(), String> {
        let p = self
            .permission_assignment(worker)
            .ok_or("No permission assignment")?;
        if p.token != token
            || p.handler != Some(handler)
            || !p.delivered
            || p.outcome.is_some()
            || now >= p.deadline_ms
            || !self.permission_handler_ready(handler)
            || self
                .agent(handler)
                .and_then(|a| a.runtime_identity.launch_id.as_ref())
                != p.generation.as_ref()
            || self
                .agent(worker)
                .and_then(|a| a.runtime_identity.launch_id.as_ref())
                != p.worker_launch.as_ref()
        {
            return Err("Permission assignment lease is stale or unavailable".into());
        }
        Ok(())
    }
    pub(crate) fn resolve_permission_dialog(&mut self, worker: AgentId, shape: &str) {
        if let Some(p) = self.permission_assignments.get_mut(&worker).filter(|p| {
            p.outcome.is_none()
                && matches!(
                    p.episode.as_str(),
                    "blocked" | crate::agents::dialog::CODEX_PENDING_QUESTION_ID
                )
        }) {
            p.episode = shape.into();
        }
    }
    pub(crate) fn permission_record_fingerprint(&mut self, worker: AgentId, fingerprint: &str) {
        if let Some(p) = self.permission_assignments.get_mut(&worker) {
            p.fingerprint = Some(fingerprint.into());
        }
    }
    pub(crate) fn permission_answer_attempt(
        &mut self,
        worker: AgentId,
        fingerprint: &str,
    ) -> Result<(), String> {
        let p = self
            .permission_assignments
            .get_mut(&worker)
            .ok_or("Unknown assignment")?;
        if p.answer_attempted {
            return Err(
                "Permission answer already attempted; never retry an uncertain answer".into(),
            );
        }
        p.answer_attempted = true;
        p.fingerprint = Some(fingerprint.into());
        Ok(())
    }
    pub(crate) fn permission_answer_confirm(&mut self, worker: AgentId) {
        if let Some(p) = self.permission_assignments.get_mut(&worker) {
            p.answer_confirmed = true;
        }
    }
    pub(crate) fn complete_permission(
        &mut self,
        worker: AgentId,
        outcome: PermissionOutcome,
        summary: &str,
        now: u64,
    ) -> Result<(), String> {
        let p = self
            .permission_assignments
            .get(&worker)
            .ok_or("Unknown assignment")?
            .clone();
        if p.outcome.is_some() {
            return Err("Permission assignment already completed".into());
        }
        if outcome == PermissionOutcome::Approved && !p.answer_confirmed {
            return Err("Approval requires a confirmed typed answer".into());
        }
        if let Some(request) = p.request.filter(|id| {
            self.request(*id)
                .is_some_and(|r| r.phase == super::RequestPhase::Queued)
        }) {
            self.recover_idle_request(request, now)
                .map_err(|e| e.to_string())?;
        }
        let summary: String = summary
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect();
        let mut completed = p.clone();
        completed.outcome = Some(outcome);
        self.permission_assignments
            .insert(worker, completed.clone());
        self.permission_decisions.push(PermissionDecision {
            assignment: completed,
            at_ms: now,
            policy_version: POLICY_VERSION.into(),
            summary: summary.clone(),
        });
        if self.permission_decisions.len() > 100 {
            self.permission_decisions.remove(0);
        }
        if p.delivered {
            if let Some(handler) = p.handler.and_then(|id| self.agents.get_mut(&id)) {
                handler.permission_reset_required = true;
                handler.permission_reset_at_ms = Some(now);
            }
        }
        if outcome == PermissionOutcome::Superseded {
            return Ok(());
        }
        let name = self
            .agent(worker)
            .map(|a| a.name.as_str())
            .unwrap_or("deleted worker");
        let text = format!("{name} (agent {}): {:?}; {summary}; Inspect: bus agent dialog {}; episode {}; fingerprint {}.", worker.0, outcome, worker.0, crate::utils::time::digest(p.episode.as_bytes()), p.fingerprint.as_ref().map(|f|crate::utils::time::digest(f.as_bytes())).as_deref().unwrap_or("unavailable"));
        let author = p
            .handler
            .filter(|id| self.agent(*id).is_some())
            .unwrap_or(worker);
        if outcome != PermissionOutcome::Approved {
            if let Some(to) = self
                .orchestrator_of(p.room)
                .filter(|a| !a.deletion_pending && a.id != worker)
                .map(|a| a.id)
            {
                self.submit_message_from(
                    p.room,
                    Draft {
                        text,
                        files: vec![],
                        recipient_ids: [to].into(),
                    },
                    Author::Agent(author),
                    now,
                )
                .map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
        // A MASTER orchestrator's fallback belongs to its work room too.
        self.post_permission_notice(p.room, author, text, now)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn post_permission_notice(
        &mut self,
        room: RoomId,
        author: AgentId,
        text: String,
        now: u64,
    ) -> Result<(), ModelError> {
        self.post_to_developer(room, author, text, vec![], now)
            .map(|_| ())
    }
}
