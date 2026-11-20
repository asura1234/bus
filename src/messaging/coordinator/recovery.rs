//! Explicit room recovery reuses saved conversations; typed tasks are never replayed.
use super::{
    schema, AgentId, AgentRuntimeIdentity, Method, Provider, ResponseResult, RoomAgent, RoomId,
    RuntimeStatus, Worker,
};
use crate::agents::providers::launch;
use crate::agents::resume::catalog::{self, AgentSessionRefKind};
use crate::messaging::identity;
use crate::messaging::model::{LaunchKind, LaunchOutcome, StartupRecovery};
use serde_json::{json, Value};

/// Old snapshots may have confirmed hooks solely from file contents. Hold
/// their work before the first poll when Codex cannot load that project layer.
pub(super) fn check_saved_project_hooks(
    state: &mut super::BusState,
    id: AgentId,
    data_dir: &std::path::Path,
) -> Result<(), String> {
    let Some(agent) = state
        .agent(id)
        .filter(|agent| agent.provider == super::Provider::Codex && !agent.deletion_pending)
    else {
        return Ok(());
    };
    let checked = (|| {
        let options = agent
            .runtime_identity
            .launch_id
            .as_ref()
            .map(|launch_id| {
                launch::saved_options(
                    crate::agents::providers::ProviderKind::Codex,
                    &data_dir.join("callbacks").join(launch_id),
                )
            })
            .transpose()?
            .unwrap_or_default();
        let project =
            crate::agents::providers::codex::launch::project_directory(&agent.cwd, &options)?;
        crate::agents::providers::codex::launch::validate_project_trust(&project)
    })();
    if let Err(error) = checked {
        state
            .invalidate_agent_session(id)
            .map_err(|e| e.to_string())?;
        state
            .set_agent_error(id, Some(error.clone()))
            .map_err(|e| e.to_string())?;
        tracing::warn!(event = "bus.hooks.unavailable", agent_id = id.0,
            reason = %error, "Saved Codex hook setup is no longer confirmed; queued work is held");
    }
    Ok(())
}

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
            let recovery = agent.startup.as_ref().and_then(|s| s.recovery.as_ref());
            let tracked = !matches!(recovery, Some(StartupRecovery::Requested));
            if tracked {
                self.begin_agent_launch(
                    agent,
                    if matches!(recovery, Some(StartupRecovery::Unsubmitted { .. })) {
                        LaunchKind::Recovery
                    } else {
                        LaunchKind::Resume
                    },
                )?;
            }
            let mut state = self.state.clone();
            if tracked {
                super::launch_metrics::settle_current(
                    &mut state,
                    agent.id,
                    LaunchOutcome::Failed,
                    Some("missing_executable"),
                    super::io::now_ms(),
                );
            }
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
            Some(StartupRecovery::CodexBootstrap { .. }) => {
                return Err("Codex startup recovery is watched; inspect its terminal before an explicit replacement".into());
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
            binary: crate::platform::fs::running_executable().map_err(|e| e.to_string())?,
        };
        let attempt = self.begin_agent_launch(agent, LaunchKind::Resume)?;
        let extras = super::resume::for_native_resume(
            &context,
            &super::resume::NativeResumeFacts {
                agent_name: Some(identity::managed_name(agent.room_id, agent.id)),
                managed_agent: Some(kind.into()),
                session: Some(session),
            },
            &plan,
        );
        let extras = self.track_launch_result(attempt, "resume_preparation_failed", extras)?;
        self.launch_saved_agent(agent, kind, plan, extras, attempt)
    }

    fn launch_saved_agent(
        &mut self,
        agent: &RoomAgent,
        kind: &str,
        plan: catalog::AgentResumePlan,
        extras: super::resume::LaunchExtras,
        attempt: u64,
    ) -> Result<(), String> {
        let prepared: Result<_, String> = (|| {
            let binding = if matches!(agent.provider, Provider::Grok | Provider::Kimi) {
                let env = extras.env.iter().cloned().collect();
                let executable =
                    launch::selected_executable(identity::provider_kind(agent.provider), &env)?
                        .ok_or("Provider resume has no selected executable")?;
                let grok_home = env.get("GROK_HOME").cloned();
                let kimi_home = env.get("KIMI_CODE_HOME").cloned();
                if agent.provider == Provider::Grok && grok_home.is_none()
                    || agent.provider == Provider::Kimi && kimi_home.is_none()
                {
                    return Err("Provider resume has no validated home".into());
                }
                (Some(executable), grok_home, kimi_home)
            } else {
                (None, None, None)
            };
            Ok(binding)
        })();
        let (executable, grok_home, kimi_home) =
            self.track_launch_result(attempt, "resume_preparation_failed", prepared)?;
        let closed = self.close_resume_terminal(agent);
        self.track_launch_result(attempt, "resume_terminal_close_failed", closed)?;
        let mut args = plan.argv[1..].to_vec();
        args.extend(extras.args);
        let created = self
            .transport
            .request(Method::TabCreate(schema::TabCreateParams {
                workspace_id: None,
                cwd: Some(agent.cwd.to_string_lossy().into()),
                focus: false,
                label: Some(agent.name.clone()),
                env: extras.env.into_iter().collect(),
            }));
        let root_pane = match created {
            Ok(ResponseResult::TabCreated { root_pane, .. }) => root_pane,
            Err(error) if error.definitely_rejected => {
                let argv = std::iter::once(plan.argv[0].clone())
                    .chain(args.iter().cloned())
                    .collect();
                return self.record_failed_launch_creation(
                    agent.id,
                    argv,
                    &error.message,
                    error.code.as_deref().unwrap_or("terminal_creation_failed"),
                );
            }
            other => {
                return Err(format!(
                    "Terminal creation outcome needs inspection; no retry. {other:?}"
                ))
            }
        };
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
        super::launch_metrics::bind_current(&mut state, agent.id);
        state
            .observe_status(agent.id, RuntimeStatus::Launching, super::io::now_ms())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.watch_resume_startup(
            agent.id,
            executable.as_deref().unwrap_or(&plan.argv[0]),
            &args,
        )?;
        self.missing_binary_checked.remove(&agent.id);
        let started = self
            .transport
            .request(Method::AgentStart(schema::AgentStartParams {
                name: identity::managed_name(agent.room_id, agent.id),
                kind: kind.into(),
                pane_id: root_pane.pane_id.clone(),
                args: args.clone(),
                executable,
                grok_home,
                kimi_home,
                timeout_ms: None,
            }));
        match started {
            Ok(ResponseResult::AgentStarted { .. }) => {}
            other => return self.record_failed_agent_start(agent.id, args, other),
        }
        self.finish_resume_start(agent, root_pane.pane_id)
    }

    fn finish_resume_start(&mut self, agent: &RoomAgent, pane_id: String) -> Result<(), String> {
        // The new launch now owns diagnosis. Keeping the previous missing
        // marker would suppress both startup settlement and a second outage.
        let mut state = self.state.clone();
        state
            .clear_missing_binary(agent.id)
            .map_err(|e| e.to_string())?;
        state
            .clear_session_restart_closed_terminal(agent.id)
            .map_err(|e| e.to_string())?;
        state
            .set_agent_error(agent.id, None)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.report_adopted_session(
            agent.id,
            agent.provider,
            pane_id,
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
        let started_at_ms = state
            .agent(id)
            .and_then(|a| a.startup.as_ref())
            .ok_or("Resume startup disappeared")?
            .started_at_ms;
        state.watch_launch_command(id, started_at_ms);
        self.save(state)
    }

    fn close_resume_terminal(&mut self, agent: &RoomAgent) -> Result<(), String> {
        if agent.runtime_identity.pane_id.is_none() && agent.runtime_identity.terminal_id.is_some()
        {
            return Ok(());
        }
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
        if agent
            .session_restart_closed_terminal
            .as_deref()
            .is_some_and(|closed| Some(closed) != agent.runtime_identity.terminal_id.as_deref())
        {
            return Ok(());
        }
        let pane = pane.clone();
        let terminal = terminal.clone();
        let closed = self.transport.request(Method::PaneCloseIfIdentity(
            schema::PaneCloseIfIdentityParams {
                pane_id: pane.clone(),
                expected_terminal_id: terminal.clone(),
                expected_agent: identity::provider_kind(agent.provider).label().into(),
                expected_managed_name: identity::managed_name(agent.room_id, agent.id),
                expected_session_id: agent.runtime_identity.session_id.clone(),
                close_exited_shell: true,
                allow_busy_shell: false,
            },
        ));
        match closed {
            Ok(ResponseResult::Ok {}) => Ok(()),
            Ok(_) => {
                self.log_unconfirmed_pane_close(
                    agent,
                    &pane,
                    &terminal,
                    None,
                    "close response was not Ok",
                );
                Err("Previous pane close is unconfirmed; no provider was launched".into())
            }
            Err(error) => {
                self.log_unconfirmed_pane_close(
                    agent,
                    &pane,
                    &terminal,
                    error.code.as_deref(),
                    &error.message,
                );
                Err(format!(
                    "Previous pane could not be safely closed: {}",
                    error.message
                ))
            }
        }
    }

    /// The read is diagnostic. It does not record a snapshot or continue resume.
    fn log_unconfirmed_pane_close(
        &mut self,
        agent: &RoomAgent,
        pane_id: &str,
        saved_terminal_id: &str,
        close_code: Option<&str>,
        close_detail: &str,
    ) {
        let current = match self.transport.request(Method::PaneGet(schema::PaneTarget {
            pane_id: pane_id.to_owned(),
        })) {
            Ok(ResponseResult::PaneInfo { pane }) => Some(pane),
            _ => None,
        };
        let (current_pane_id, current_terminal_id, current_agent, current_session) =
            match current.as_ref() {
                Some(pane) => (
                    pane.pane_id.as_str(),
                    pane.terminal_id.as_str(),
                    pane.agent.as_deref().unwrap_or(""),
                    pane.agent_session
                        .as_ref()
                        .map(|session| session.value.as_str())
                        .unwrap_or(""),
                ),
                None => ("unreadable", "unreadable", "unreadable", "unreadable"),
            };
        let expected_managed_name = identity::managed_name(agent.room_id, agent.id);
        let expected_session_id = agent.runtime_identity.session_id.as_deref().unwrap_or("");
        tracing::warn!(
            event = "bus.resume.pane_close_unconfirmed",
            pane_id,
            saved_terminal_id,
            current_identity_readable = current.is_some(),
            current_pane_id,
            current_terminal_id,
            current_agent,
            current_session,
            expected_agent = identity::provider_kind(agent.provider).label(),
            expected_managed_name = expected_managed_name.as_str(),
            expected_session_id,
            close_code = close_code.unwrap_or(""),
            close_detail,
            "Previous pane close is unconfirmed"
        );
    }
}
