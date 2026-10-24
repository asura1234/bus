//! Explicit room recovery reuses saved conversations; typed tasks are never replayed.
use super::{
    schema, AgentId, AgentRuntimeIdentity, Method, ResponseResult, RoomAgent, RoomId,
    RuntimeStatus, Worker,
};
use crate::agents::providers::launch;
use crate::agents::resume::catalog::{self, AgentSessionRefKind};
use crate::messaging::identity;
use serde_json::{json, Value};

impl Worker {
    pub(super) fn resume_room(&mut self, room: RoomId) -> Result<Value, String> {
        // A fresh native list prevents relaunching a provider which recovered on its own.
        let infos = self
            .poll()?
            .ok_or("Cannot verify native agents; no resume attempted")?;
        let targets: Vec<_> = self
            .state
            .agents()
            .filter(|a| a.room_id == room && !a.deletion_pending)
            .filter(|a| a.status == RuntimeStatus::Unavailable || a.missing_binary.is_some())
            .cloned()
            .collect();
        let mut results = Vec::new();
        for agent in targets {
            let mut row = json!({"agent_id":agent.id, "name":agent.name});
            let name = identity::managed_name(room, agent.id);
            if infos.iter().any(|i| {
                i.name.as_deref() == Some(&name) && (i.interactive_ready || i.launch_pending)
            }) {
                row["outcome"] = json!("skipped");
                row["reason"] = json!("provider is running or still launching");
            } else {
                match self.resume_saved_agent(&agent) {
                    Ok(()) => row["outcome"] = json!("resuming"),
                    Err(error) => {
                        row["outcome"] = json!("blocked");
                        row["reason"] = json!(error);
                    }
                }
            }
            results.push(row);
        }
        let recovered: Vec<_> = self
            .state
            .room(room)
            .ok_or("Unknown room")?
            .missing_provider_binaries
            .iter()
            .filter(|binary| launch::executable_available(binary))
            .cloned()
            .collect();
        let mut state = self.state.clone();
        for binary in recovered {
            state.clear_unheld_missing_binary(room, &binary);
        }
        self.save(state)?;
        Ok(json!({"room_id":room, "agents":results}))
    }

    fn resume_saved_agent(&mut self, agent: &RoomAgent) -> Result<(), String> {
        if let Err(error) = self.ensure_provider_binary(agent.room_id, agent.provider) {
            let mut state = self.state.clone();
            state
                .block_missing_binary(agent.id, agent.provider.executable(), super::io::now_ms())
                .map_err(|e| e.to_string())?;
            self.save(state)?;
            return Err(error);
        }
        if agent.session_binding_invalidated {
            return Err("Session ownership or hook setup needs inspection before resume".into());
        }
        match agent
            .startup
            .as_ref()
            .and_then(|startup| startup.recovery.as_ref())
        {
            Some(crate::messaging::model::StartupRecovery::Unsubmitted { args }) => {
                return self.retry_unsubmitted_agent_start(agent, args.clone());
            }
            Some(crate::messaging::model::StartupRecovery::Requested) => {
                return Err("Previous launch input is unconfirmed; inspect the owned terminal before recovery; no retry was sent".into());
            }
            None => {}
        }
        if !agent.hook_setup_confirmed {
            return Err("Session ownership or hook setup needs inspection before resume".into());
        }
        let kind = identity::provider_kind(agent.provider).label();
        let session = catalog::session_ref_from_snapshot(
            &format!("herdr:{kind}"),
            kind,
            AgentSessionRefKind::Id,
            agent
                .runtime_identity
                .session_id
                .as_deref()
                .ok_or("No saved provider session")?,
        )
        .ok_or("Invalid saved provider session")?;
        let plan = catalog::plan(&session.source, kind, &session.session_ref)
            .ok_or("No supported resume command for this session")?;
        let context = super::resume::NativeResumeContext {
            root: Some(self.data_dir.clone()),
            session_name: Some(super::DEFAULT_SESSION.into()),
            config_root: self.data_dir.join("herdr-config"),
            project: crate::agents::providers::launch::project_root(&agent.cwd),
            binary: std::env::current_exe().map_err(|e| e.to_string())?,
        };
        let extras = super::resume::for_native_resume(
            &context,
            &super::resume::NativeResumeFacts {
                agent_name: Some(identity::managed_name(agent.room_id, agent.id)),
                managed_agent: Some(kind.into()),
                session: Some(session),
            },
            &plan,
        )?;
        self.launch_saved_agent(agent, kind, plan, extras)
    }

    fn launch_saved_agent(
        &mut self,
        agent: &RoomAgent,
        kind: &str,
        plan: catalog::AgentResumePlan,
        extras: super::resume::LaunchExtras,
    ) -> Result<(), String> {
        self.close_resume_terminal(agent)?;
        let created = self
            .transport
            .request(Method::TabCreate(schema::TabCreateParams {
                workspace_id: None,
                cwd: Some(agent.cwd.to_string_lossy().into()),
                focus: false,
                label: Some(agent.name.clone()),
                env: extras.env.into_iter().collect(),
            }))
            .map_err(|e| e.message)?;
        let ResponseResult::TabCreated { root_pane, .. } = created else {
            return Err("Terminal creation outcome needs inspection; no retry".into());
        };
        let mut args = plan.argv[1..].to_vec();
        args.extend(extras.args);
        let mut state = self.state.clone();
        state
            .set_agent_runtime_identity(
                agent.id,
                AgentRuntimeIdentity {
                    terminal_id: Some(root_pane.terminal_id),
                    pane_id: Some(root_pane.pane_id.clone()),
                    ..agent.runtime_identity.clone()
                },
            )
            .map_err(|e| e.to_string())?;
        state
            .observe_status(agent.id, RuntimeStatus::Launching, super::io::now_ms())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.watch_resume_startup(agent.id, &plan.argv[0], &args)?;
        self.missing_binary_checked.remove(&agent.id);
        let started = self
            .transport
            .request(Method::AgentStart(schema::AgentStartParams {
                name: identity::managed_name(agent.room_id, agent.id),
                kind: kind.into(),
                pane_id: root_pane.pane_id.clone(),
                args,
                timeout_ms: None,
            }))
            .map_err(|e| e.message)?;
        if !matches!(started, ResponseResult::AgentStarted { .. }) {
            return Err("Provider start outcome needs inspection; no retry".into());
        }
        // The new launch now owns diagnosis. Keeping the previous missing
        // marker would suppress both startup settlement and a second outage.
        let mut state = self.state.clone();
        state
            .clear_missing_binary(agent.id)
            .map_err(|e| e.to_string())?;
        state
            .set_agent_error(agent.id, None)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.report_adopted_session(
            agent.id,
            agent.provider,
            root_pane.pane_id,
            agent
                .runtime_identity
                .session_id
                .clone()
                .ok_or("Saved session disappeared")?,
        )
    }

    fn watch_resume_startup(
        &mut self,
        id: AgentId,
        executable: &str,
        args: &[String],
    ) -> Result<(), String> {
        let argv = std::iter::once(executable.to_owned())
            .chain(args.iter().cloned())
            .collect();
        self.watch_agent_startup(id, argv)?;
        let mut state = self.state.clone();
        let mut startup = state
            .agent(id)
            .and_then(|a| a.startup.clone())
            .ok_or("Resume startup was not saved")?;
        startup.started_at_ms = super::io::now_ms();
        state
            .set_agent_startup(id, Some(startup))
            .map_err(|e| e.to_string())?;
        self.save(state)
    }

    fn close_resume_terminal(&mut self, agent: &RoomAgent) -> Result<(), String> {
        let (Some(pane), Some(terminal)) = (
            &agent.runtime_identity.pane_id,
            &agent.runtime_identity.terminal_id,
        ) else {
            return Err("Saved terminal identity is incomplete".into());
        };
        if agent
            .exit_snapshot
            .as_ref()
            .is_some_and(|s| s.terminal_closed == Some(true) && &s.terminal_id == terminal)
        {
            return Ok(());
        }
        let closed = self
            .transport
            .request(Method::PaneCloseIfIdentity(
                schema::PaneCloseIfIdentityParams {
                    pane_id: pane.clone(),
                    expected_terminal_id: terminal.clone(),
                    expected_agent: identity::provider_kind(agent.provider).label().into(),
                    expected_managed_name: identity::managed_name(agent.room_id, agent.id),
                    expected_session_id: agent.runtime_identity.session_id.clone(),
                    close_exited_shell: true,
                    allow_busy_shell: false,
                },
            ))
            .map_err(|e| format!("Previous pane could not be safely closed: {}", e.message))?;
        if matches!(closed, ResponseResult::Ok {}) {
            Ok(())
        } else {
            Err("Previous pane close is unconfirmed; no provider was launched".into())
        }
    }
}
