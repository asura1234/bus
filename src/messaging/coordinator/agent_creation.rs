//! Agent creation preflight and durable launch publication.
use super::agents::first_message_text;
use super::{
    mpsc, schema, AddAgent, AgentId, AgentRuntimeIdentity, Author, BusEvent, Draft, Method,
    Provider, ResponseResult, RoomId, RoomKind, Worker, MASTER_AGENT_NEEDS_ROOM,
};
use crate::agents::providers::{codex, hook_json::HookContext, launch, spool};
use crate::messaging::identity;
use crate::messaging::orchestration::{self as orchestrator, OrchestratorSpec};

type LaunchBinding = (Option<String>, Option<String>, Option<String>);

fn launch_binding(
    state: &mut super::BusState,
    id: AgentId,
    input: &AddAgent,
    prepared: &launch::PreparedLaunch,
) -> Result<LaunchBinding, String> {
    let _ = state.agent(id).ok_or("Unknown launching agent")?;
    let provider = input.provider;
    let env = &prepared.env;
    let executable = launch::selected_executable(provider, env)?;
    let home = if provider == Provider::Grok {
        Some(
            env.get("GROK_HOME")
                .cloned()
                .ok_or("Grok launch has no validated home")?,
        )
    } else {
        None
    };
    let kimi_home = if provider == Provider::Kimi {
        Some(
            env.get("KIMI_CODE_HOME")
                .cloned()
                .ok_or("Kimi launch has no private home")?,
        )
    } else {
        None
    };
    Ok((executable, home, kimi_home))
}

fn bind_launch_orchestrator(
    state: &mut super::BusState,
    id: AgentId,
    room: Option<RoomId>,
) -> Result<(), String> {
    if let Some(room) = room {
        state
            .bind_orchestrator(id, room)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Worker {
    pub(super) fn add_agent(
        &mut self,
        input: AddAgent,
        orchestrator: Option<OrchestratorSpec>,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let orchestrates = orchestrator.as_ref().map(|spec| spec.room);
        let cwd = self.validate_agent_addition(&input, orchestrator.as_ref())?;
        let mut state = self.state.clone();
        let id = state
            .create_agent(input.room, &input.name, input.provider, cwd.clone(), None)
            .map_err(|e| e.to_string())?;
        // Validate the assignment before any consent prompt or launch side effect.
        bind_launch_orchestrator(&mut state, id, orchestrates)?;
        if !input.consent_project_hooks {
            let binary = self.hook_callback_binary(input.provider)?;
            if let Some(notice) =
                launch::setup_notice(identity::provider_kind(input.provider), &cwd, &binary)
            {
                let _ = events.send(BusEvent::SetupRequired {
                    input,
                    orchestrator,
                    notice,
                });
                return Ok(());
            }
        }
        let attempt = self.begin_tracked_launch(
            &input.name,
            input.provider,
            input.room,
            crate::messaging::model::LaunchKind::Create,
        )?;
        state.copy_launch_ledger_from(&self.state);
        let checked = self.ensure_provider_binary(input.room, input.provider);
        self.track_launch_result(attempt, "missing_executable", checked)?;
        let prepared = self.prepare_provider_launch(&input, id);
        let (mut prepared, hooks) =
            self.track_launch_result(attempt, "preparation_failed", prepared)?;
        let prompt = self.prepare_agent_prompt(
            &input,
            id,
            &mut state,
            orchestrator.as_ref(),
            &mut prepared,
            &hooks,
        );
        self.track_launch_result(attempt, "prompt_preparation_failed", prompt)?;
        let binding = launch_binding(&mut state, id, &input, &prepared);
        let (selected_executable, grok_home, kimi_home) =
            self.track_launch_result(attempt, "launch_binding_failed", binding)?;
        state
            .attach_launch_attempt(attempt, id)
            .map_err(|e| e.to_string())?;
        let identity = self.persist_agent_launch(
            state,
            id,
            input.provider,
            hooks.launch_id,
            prepared.adopted_session.clone(),
        )?;
        let created = self.create_agent_tab(&input, &mut prepared);
        let (pane, terminal) = match created {
            Ok(ResponseResult::TabCreated { root_pane, .. }) => {
                (root_pane.pane_id, root_pane.terminal_id)
            }
            Err(error) if error.definitely_rejected => {
                let argv = std::iter::once(launch::executable(input.provider).to_owned())
                    .chain(prepared.args.iter().cloned())
                    .collect();
                return self.record_failed_launch_creation(
                    id,
                    argv,
                    &error.message,
                    error.code.as_deref().unwrap_or("terminal_creation_failed"),
                );
            }
            other => {
                return self.agent_error(
                    id,
                    format!("Tab creation outcome requires inspection; no retry. {other:?}"),
                )
            }
        };
        let mut state = self.state.clone();
        state
            .set_agent_runtime_identity(
                id,
                AgentRuntimeIdentity {
                    pane_id: Some(pane.clone()),
                    terminal_id: Some(terminal),
                    ..identity.clone()
                },
            )
            .map_err(|e| e.to_string())?;
        super::launch_metrics::bind_current(&mut state, id);
        self.save(state)?;
        // This identity is durable before start. A missing response must not launch twice.
        let mut argv = vec![selected_executable
            .clone()
            .unwrap_or_else(|| launch::executable(input.provider).to_owned())];
        argv.extend(prepared.args.iter().cloned());
        self.watch_agent_startup(id, argv)?;
        let started = self
            .transport
            .request(Method::AgentStart(schema::AgentStartParams {
                name: identity::managed_name(input.room, id),
                kind: identity::provider_kind(input.provider).label().into(),
                pane_id: pane.clone(),
                args: prepared.args.clone(),
                executable: selected_executable,
                grok_home,
                kimi_home,
                timeout_ms: None,
            }));
        match started {
            Ok(ResponseResult::AgentStarted { .. }) => {
                let _ = events.send(BusEvent::AgentAdded(id));
                let Some(session) = identity.session_id else {
                    return Ok(());
                };
                self.report_adopted_session(id, input.provider, pane, session)
            }
            other => self.record_failed_agent_start(id, prepared.args, other),
        }
    }

    fn create_agent_tab(
        &mut self,
        input: &AddAgent,
        prepared: &mut launch::PreparedLaunch,
    ) -> Result<ResponseResult, crate::messaging::native::TransportError> {
        self.transport
            .request(Method::TabCreate(schema::TabCreateParams {
                workspace_id: None,
                cwd: Some(prepared.cwd.to_string_lossy().into_owned()),
                focus: false,
                label: Some(input.name.clone()),
                env: std::mem::take(&mut prepared.env),
            }))
    }

    /// Codex hashes the command string, so the callback path is a data-dir
    /// bridge. Consent must inspect that same path, or a trusted set looks new.
    fn hook_callback_binary(&self, provider: Provider) -> Result<std::path::PathBuf, String> {
        let current = std::env::current_exe().map_err(|e| e.to_string())?;
        if provider == Provider::Codex {
            crate::agents::providers::hook_json::codex_callback_binary(&self.data_dir, &current)
        } else {
            Ok(current)
        }
    }

    /// Bus owns messaging identity and routing; providers receive capture facts.
    fn prepare_provider_launch(
        &self,
        input: &AddAgent,
        id: AgentId,
    ) -> Result<(launch::PreparedLaunch, HookContext), String> {
        let binary = self.hook_callback_binary(input.provider)?;
        let launch_id = crate::utils::time::digest(
            format!(
                "{}:{}:{}",
                id.0,
                std::process::id(),
                crate::utils::time::now_ns()
            )
            .as_bytes(),
        );
        let hooks = HookContext {
            binary,
            spool: self.data_dir.join("callbacks").join(&launch_id),
            launch_id,
        };
        let spec = launch::LaunchSpec {
            provider: identity::provider_kind(input.provider),
            cwd: input.cwd.clone(),
            extra_args: input.extra_args.clone(),
            consent_project_hooks: input.consent_project_hooks,
            hooks: hooks.clone(),
        };
        let prepared = launch::prepare(&spec, |context| {
            let manifest = spool::Manifest {
                routing_key: spool::RoutingKey(id.0),
                provider: spec.provider,
                launch_id: context.launch_id.clone(),
            };
            spool::initialize(&context.spool, &manifest).map_err(|e| e.to_string())
        })?;
        Ok((prepared, hooks))
    }

    fn persist_agent_launch(
        &mut self,
        mut state: super::BusState,
        id: AgentId,
        provider: Provider,
        launch_id: String,
        adopted_session: Option<String>,
    ) -> Result<AgentRuntimeIdentity, String> {
        // cursor-agent --resume fires no sessionStart; its first hook comes with
        // the first prompt, which Bus types only into a bound session. The
        // session Bus itself launched with binds it, and every later callback
        // must still match it.
        let session_id = adopted_session.filter(|_| provider == Provider::Cursor);
        let identity = AgentRuntimeIdentity {
            launch_id: Some(launch_id),
            session_id,
            ..Default::default()
        };
        state
            .set_agent_runtime_identity(id, identity.clone())
            .map_err(|e| e.to_string())?;
        super::launch_metrics::bind_current(&mut state, id);
        if matches!(provider, Provider::ClaudeCode | Provider::Kimi)
            || launch::provider_kind(provider) == "fake"
        {
            state.confirm_hook_setup(id).map_err(|e| e.to_string())?;
        }
        state
            .set_agent_error(
                id,
                Some(
                    match provider {
                        Provider::Codex | Provider::Cursor | Provider::Grok | Provider::Kimi => {
                            "Not ready; waiting for the provider terminal."
                        }
                        Provider::ClaudeCode => "Not ready; waiting for provider hook readiness.",
                        #[cfg(feature = "dev-tools")]
                        Provider::Fake => "Not ready; waiting for provider hook readiness.",
                    }
                    .into(),
                ),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(identity)
    }

    pub(super) fn report_adopted_session(
        &mut self,
        id: AgentId,
        provider: Provider,
        pane: String,
        session: String,
    ) -> Result<(), String> {
        // What a SessionStart hook would report, so the native layer's
        // identity checks for dialogs and delivery accept this pane.
        let kind = identity::provider_kind(provider).label();
        match self.transport.request(Method::PaneReportAgentSession(
            schema::PaneReportAgentSessionParams {
                pane_id: pane,
                source: format!("herdr:{kind}"),
                agent: kind.into(),
                seq: None,
                agent_session_id: Some(session),
                agent_session_path: None,
                session_start_source: Some("resume".into()),
                reporter: Vec::new(),
            },
        )) {
            Ok(_) => Ok(()),
            Err(error) => self.agent_error(
                id,
                format!(
                    "Adopted session not reported to its terminal: {}",
                    error.message
                ),
            ),
        }
    }

    pub(super) fn prepare_agent_prompt(
        &self,
        input: &AddAgent,
        id: AgentId,
        state: &mut super::BusState,
        spec: Option<&OrchestratorSpec>,
        prepared: &mut launch::PreparedLaunch,
        hooks: &HookContext,
    ) -> Result<(), String> {
        let is_orchestrator = spec.is_some();
        let docs = orchestrator::write_docs(&self.data_dir)?;
        let name = state.agent(id).map(|a| a.name.as_str()).unwrap_or_default();
        let onboarding = orchestrator::agent_onboarding(name, id, input.room, &docs);
        let text = if let Some(spec) = spec {
            let values = orchestrator::PromptValues {
                room: state
                    .room(spec.room)
                    .map(|room| (room.name.clone(), room.id)),
                agent: name.into(),
                docs,
            };
            let template = spec
                .system_prompt
                .as_deref()
                .unwrap_or(orchestrator::DEFAULT_PROMPT);
            format!("{onboarding}\n\n{}", orchestrator::fill(template, &values))
        } else {
            onboarding
        };
        let text = if input.provider == Provider::Grok && is_orchestrator {
            crate::agents::providers::grok::system_prompt::orchestrator_prompt(&text)?
        } else if input.provider == Provider::Kimi && spec.is_some() {
            format!(
                "{}\n\n{text}",
                include_str!("../../../orchestration/providers/kimi.md")
            )
        } else {
            text
        };
        let adopted = prepared.adopted_session.is_some();
        let path = orchestrator::write_prompt(&hooks.spool, &text)?;
        match orchestrator::prompt_args(input.provider, &path, adopted)? {
            Some(args) => launch::extend_generated_args(
                identity::provider_kind(input.provider),
                &mut prepared.args,
                args,
            ),
            None if matches!(input.provider, Provider::Cursor | Provider::Grok) => {}
            None => {
                let master = input.room;
                state
                    .submit_bus_message_with(
                        master,
                        Draft {
                            text: first_message_text(input.provider, is_orchestrator, &text),
                            files: Vec::new(),
                            recipient_ids: [id].into_iter().collect(),
                        },
                        Author::Developer,
                        crate::messaging::storage::io::now_ms(),
                        // The system prompt is the agent's first turn of its own.
                        true,
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    fn validate_agent_addition(
        &self,
        input: &AddAgent,
        orchestrator: Option<&OrchestratorSpec>,
    ) -> Result<std::path::PathBuf, String> {
        let orchestrates = orchestrator.map(|spec| spec.room);
        // Before anything else, so the CLI and the form get the same answer.
        if orchestrator.is_some() {
            orchestrator::check_new_orchestrator(input.provider)?;
        }
        let cwd = launch::canonical_directory(&input.cwd)?;
        if input.provider == Provider::Codex {
            codex::launch::validate_project_trust(&cwd)?;
        }
        // One provider session belongs to one Bus agent: bound by its hook, or
        // reserved by a launch that adopted it and has not reported yet.
        if let Some(session) =
            launch::adopted_session(identity::provider_kind(input.provider), &input.extra_args)?
        {
            if let Some(owner) = self.state.agents().find(|agent| {
                let identity = &agent.runtime_identity;
                match identity.session_id.as_deref() {
                    Some(bound) => bound == session,
                    // The reservation outlives its launch's first bind, so a
                    // bound agent (rebound by clear, too) owns only that session.
                    None => identity.launch_id.as_ref().is_some_and(|launch| {
                        launch::reserved_session(&self.data_dir.join("callbacks").join(launch))
                            .is_some_and(|reserved| reserved == session)
                    }),
                }
            }) {
                return Err(format!(
                    "Session {session} already belongs to Bus agent {}",
                    owner.name
                ));
            }
        }
        // Every MASTER agent is an orchestrator bound to one work room.
        if orchestrates.is_none()
            && self
                .state
                .room(input.room)
                .is_some_and(|room| room.kind == RoomKind::Master)
        {
            return Err(MASTER_AGENT_NEEDS_ROOM.into());
        }
        Ok(cwd)
    }
}
