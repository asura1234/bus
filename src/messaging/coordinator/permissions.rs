//! Managed Claude service lifecycle; persisted leases are the answer authority.
use super::{AddAgent, AgentId, BusEvent, Provider, RoomId, RoomKind, RuntimeStatus, Worker};
use crate::messaging::model::PermissionOutcome;
use crate::messaging::storage::io;
use std::path::Path;

pub(super) const POLICY: &str = include_str!("../../../orchestration/permission-handler.md");
pub(super) fn handler_prompt(room: RoomId, handler: AgentId, data: &Path) -> String {
    let executable = std::env::current_exe().ok();
    handler_prompt_with_binary(room, handler, data, executable.as_deref())
}
pub(super) fn handler_prompt_with_binary(
    room: RoomId,
    handler: AgentId,
    data: &Path,
    executable: Option<&Path>,
) -> String {
    let target = executable.and_then(|p| {
        p.ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == "target"))
    });
    let prefix = executable
        .map(crate::agents::providers::launch::profile::cli_prefix)
        .unwrap_or_else(|| "UNAVAILABLE_BUS_EXECUTABLE".into());
    let policy = POLICY.replace("bus ", &format!("{prefix} "));
    format!("{policy}\nOperational CLI prefix: {prefix}. Invoke this exact quoted executable prefix for every Bus operation. Your room is {}. Your handler agent ID is {}. Live session data/control path to protect: {}. Protected running Bus executable: {:?}; protected live build target: {:?}. Assigned worker roots must be inspected from current Bus state. Service scratch: {}.\n", room.0, handler.0, data.display(), executable, target, data.join("permission-handlers").join(room.0.to_string()).display())
}
impl Worker {
    pub(super) fn maintain_permission_handlers(&mut self) -> Result<(), String> {
        let rooms: Vec<_> = self
            .state
            .rooms()
            .filter(|r| {
                r.kind == RoomKind::Work
                    && !r.deletion_pending
                    && (self.state.agents().any(|a| {
                        a.room_id == r.id
                            && !a.permission_handler
                            && !a.deletion_pending
                            && a.runtime_identity.launch_id.is_some()
                            && a.status != RuntimeStatus::Unavailable
                    }) || self.state.permission_assignments().any(|p| p.room == r.id))
                    && self.state.permission_handler_of(r.id).is_none()
            })
            .map(|r| r.id)
            .collect();
        for room in rooms {
            let scratch = self
                .data_dir
                .join("permission-handlers")
                .join(room.0.to_string());
            io::private_dir(&scratch).map_err(|e| e.to_string())?;
            let (events, _) = std::sync::mpsc::channel::<BusEvent>();
            // The persisted role exists before launch, including launch failure;
            // reconciliation never duplicates an uncertain native start.
            if let Err(error) = self.add_agent_owned(
                AddAgent {
                    room,
                    name: format!("bus-service-{}", room.0),
                    provider: Provider::ClaudeCode,
                    cwd: scratch.to_string_lossy().into_owned(),
                    extra_args: String::new(),
                    consent_project_hooks: true,
                },
                None,
                true,
                &events,
            ) {
                tracing::warn!(event="bus.permission.launch_failed", room_id=room.0, %error);
            }
        }
        Ok(())
    }
    pub(super) fn maintain_permissions(&mut self) -> Result<(), String> {
        let now = io::now_ms();
        let pending: Vec<_> = self
            .state
            .permission_assignments()
            .filter(|p| p.outcome.is_none())
            .cloned()
            .collect();
        for p in pending {
            if self
                .state
                .room(p.room)
                .is_none_or(|room| room.deletion_pending)
            {
                let mut state = self.state.clone();
                state.complete_permission(
                    p.worker,
                    PermissionOutcome::Superseded,
                    "work room deleted or deleting",
                    now,
                )?;
                self.save(state)?;
                continue;
            }
            let reason = if now >= p.deadline_ms {
                Some("permission handler deadline expired")
            } else if p
                .handler
                .is_none_or(|id| !self.state.permission_handler_ready(id))
            {
                Some("permission handler unavailable")
            } else if p
                .handler
                .and_then(|id| self.state.agent(id))
                .is_some_and(|a| a.delivery_rejection.is_some())
            {
                Some("permission handler delivery rejected")
            } else if self.state.agent(p.worker).is_none_or(|a| {
                a.runtime_identity.launch_id != p.worker_launch || a.deletion_pending
            }) {
                Some("worker launch changed or deleted")
            } else {
                None
            };
            if let Some(reason) = reason {
                self.permission_fallback(p.worker, reason)?;
            }
        }
        let mut state = self.state.clone();
        state.prune_permission_assignments();
        if state != self.state {
            self.save(state)?;
        }
        self.maintain_permission_resets(now)?;
        let waiting: Vec<_> = self
            .state
            .permission_assignments()
            .filter(|p| p.outcome.is_none() && !p.delivered)
            .map(|p| p.worker)
            .collect();
        for worker in waiting {
            let mut state = self.state.clone();
            state.deliver_permission(worker, now)?;
            if state != self.state {
                self.save(state)?;
            }
        }
        Ok(())
    }
    fn maintain_permission_resets(&mut self, now: u64) -> Result<(), String> {
        let handlers: Vec<_> = self
            .state
            .agents()
            .filter(|a| a.permission_handler && a.permission_reset_required)
            .cloned()
            .collect();
        for handler in handlers {
            if handler
                .permission_reset_at_ms
                .is_some_and(|at| now >= at.saturating_add(90_000))
            {
                let mut state = self.state.clone();
                state
                    .set_agent_error(
                        handler.id,
                        Some("Permission handler reset failed or timed out".into()),
                    )
                    .map_err(|e| e.to_string())?;
                self.save(state)?;
                continue;
            }
            if handler.session_reset_pending {
                continue;
            }
            if handler.permission_clear_started
                && handler.status == RuntimeStatus::Idle
                && handler.hook_setup_confirmed
            {
                let mut state = self.state.clone();
                state
                    .finish_permission_reset(handler.id)
                    .map_err(|e| e.to_string())?;
                self.save(state)?;
                continue;
            }
            if handler.status == RuntimeStatus::Idle
                && handler.current_request.is_none()
                && self.state.next_queued_request(handler.id).is_none()
            {
                match self.control_clear(handler.id) {
                    Ok(_) => {
                        let mut state = self.state.clone();
                        state
                            .permission_reset_started(handler.id, now)
                            .map_err(|e| e.to_string())?;
                        self.save(state)?;
                    }
                    Err(error) => {
                        let mut state = self.state.clone();
                        state
                            .set_agent_error(
                                handler.id,
                                Some(format!("Permission handler clear failed: {error}")),
                            )
                            .map_err(|e| e.to_string())?;
                        self.save(state)?;
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn permission_fallback(
        &mut self,
        worker: AgentId,
        reason: &str,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        state.complete_permission(worker, PermissionOutcome::Fallback, reason, io::now_ms())?;
        self.save(state)
    }
}
