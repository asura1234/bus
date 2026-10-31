//! Explicit recovery of a provider command rejected before input, retaining its pane.
use super::{
    io, schema, AgentRuntimeIdentity, Method, Provider, ResponseResult, RoomAgent, RuntimeStatus,
    Worker,
};
use crate::messaging::{identity, model::StartupRecovery};

impl Worker {
    pub(super) fn retry_unsubmitted_agent_start(
        &mut self,
        agent: &RoomAgent,
        args: Vec<String>,
    ) -> Result<(), String> {
        let AgentRuntimeIdentity {
            pane_id: Some(pane),
            terminal_id: Some(terminal),
            ..
        } = &agent.runtime_identity
        else {
            return Err("Saved launch terminal identity is incomplete; no retry was sent".into());
        };
        let (executable, grok_home, kimi_home) =
            if matches!(agent.provider, Provider::Grok | Provider::Kimi) {
                let launch_id = agent
                    .runtime_identity
                    .launch_id
                    .as_deref()
                    .ok_or("Saved provider launch identity is missing; no retry was sent")?;
                let hooks = crate::agents::providers::hook_json::HookContext {
                    binary: std::env::current_exe().map_err(|e| e.to_string())?,
                    spool: self.data_dir.join("callbacks").join(launch_id),
                    launch_id: launch_id.into(),
                };
                let env = if agent.provider == Provider::Grok {
                    crate::agents::providers::grok::launch::environment(&hooks)?
                } else {
                    crate::agents::providers::kimi::launch::environment(&hooks)?
                };
                let executable = crate::agents::providers::launch::selected_executable(
                    identity::provider_kind(agent.provider),
                    &env,
                )?
                .ok_or("Saved provider executable is missing; no retry was sent")?;
                let grok_home = env.get("GROK_HOME").cloned();
                let kimi_home = env.get("KIMI_CODE_HOME").cloned();
                if agent.provider == Provider::Grok && grok_home.is_none()
                    || agent.provider == Provider::Kimi && kimi_home.is_none()
                {
                    return Err("Saved provider home is missing; no retry was sent".into());
                }
                (Some(executable), grok_home, kimi_home)
            } else {
                (None, None, None)
            };
        let mut state = self.state.clone();
        let mut startup = state
            .agent(agent.id)
            .and_then(|agent| agent.startup.clone())
            .ok_or("Saved unsubmitted launch is missing")?;
        // Consume the only retry authority durably before the native request.
        // A crash or lost response must never make that input replayable.
        startup.recovery = Some(StartupRecovery::Requested);
        startup.started_at_ms = io::now_ms();
        startup.reported = false;
        state
            .set_agent_startup(agent.id, Some(startup))
            .map_err(|e| e.to_string())?;
        state
            .set_agent_error(agent.id, None)
            .map_err(|e| e.to_string())?;
        state
            .observe_status(agent.id, RuntimeStatus::Launching, io::now_ms())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.missing_binary_checked.remove(&agent.id);
        let name = identity::managed_name(agent.room_id, agent.id);
        let result = self.transport.request(Method::AgentStartIfIdentity(
            schema::AgentStartIfIdentityParams {
                expected_terminal_id: terminal.clone(),
                start: schema::AgentStartParams {
                    name: name.clone(),
                    kind: identity::provider_kind(agent.provider).label().into(),
                    pane_id: pane.clone(),
                    args: args.clone(),
                    executable,
                    grok_home,
                    kimi_home,
                    timeout_ms: None,
                },
            },
        ));
        match result {
            Ok(ResponseResult::AgentStarted { agent: info, .. })
                if info.terminal_id == *terminal
                    && info.pane_id == *pane
                    && info.name.as_deref() == Some(&name) => {}
            other => return self.record_failed_agent_start(agent.id, args, other),
        }
        let mut state = self.state.clone();
        state
            .clear_missing_binary(agent.id)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        if let Some(session) = agent.runtime_identity.session_id.clone() {
            self.report_adopted_session(agent.id, agent.provider, pane.clone(), session)?;
        }
        Ok(())
    }
}
