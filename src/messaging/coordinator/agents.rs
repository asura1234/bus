//! Owned agent launch, deletion and terminal identity checks.
use super::{
    mpsc, schema, AgentId, AgentRuntimeIdentity, Author, BusEvent, Draft, LeftOpenTerminal, Method,
    Provider, ResponseResult, RoomId, RoomKind, Worker, MASTER_AGENT_NEEDS_ROOM,
};
use crate::agents::providers::{cursor, hook_json::HookContext, launch, spool};
use crate::messaging::identity;
use crate::messaging::orchestration::{self as orchestrator, OrchestratorSpec};

/// Room-level launch request; provider preparation receives only LaunchSpec.
#[derive(Clone, Debug)]
pub(crate) struct AddAgent {
    pub(crate) room: RoomId,
    pub(crate) name: String,
    pub(crate) provider: Provider,
    pub(crate) cwd: String,
    pub(crate) extra_args: String,
    pub(crate) consent_project_hooks: bool,
}

impl Worker {
    pub(super) fn delete_agent(
        &mut self,
        id: AgentId,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        state.prepare_delete_agent(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        let left_open: Vec<_> = self.stop_agent_terminal(id)?.into_iter().collect();
        let mut state = self.state.clone();
        state.delete_agent(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        self.branch_checks.remove(&id);
        self.finish_deletion(left_open, events);
        Ok(())
    }

    pub(super) fn delete_room(
        &mut self,
        id: RoomId,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        state.prepare_delete_room(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        // The room's orchestrator in MASTER goes with it: it exists only for
        // this room, so its terminal is stopped like a member's.
        let agents = self.state.agents_deleted_with_room(id);
        let mut left_open = Vec::new();
        for agent in &agents {
            left_open.extend(self.stop_agent_terminal(*agent)?);
        }
        let mut state = self.state.clone();
        state.delete_room(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        for agent in agents {
            self.branch_checks.remove(&agent);
        }
        self.finish_deletion(left_open, events);
        Ok(())
    }

    /// A completed deletion clears the previous failure. A terminal Bus no
    /// longer owns stays open and is reported rather than silently forgotten.
    fn finish_deletion(
        &mut self,
        left_open: Vec<LeftOpenTerminal>,
        events: &mpsc::Sender<BusEvent>,
    ) {
        self.error = (!left_open.is_empty()).then(|| {
            left_open
                .iter()
                .map(|terminal| {
                    format!(
                        "Deleted agent \"{}\". Bus no longer owns its terminal (pane {}, {}), so it was left open; close it yourself if it is no longer needed.",
                        terminal.agent_name, terminal.pane_id, terminal.terminal_id
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        });
        if !left_open.is_empty() {
            let _ = events.send(BusEvent::TerminalsLeftOpen(left_open));
        }
    }

    /// Returns the terminal that was left open because the native server no
    /// longer attributes any terminal to this agent's managed name.
    fn stop_agent_terminal(&mut self, id: AgentId) -> Result<Option<LeftOpenTerminal>, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let identity = &agent.runtime_identity;
        let (Some(pane), Some(terminal)) = (&identity.pane_id, &identity.terminal_id) else {
            if identity == &AgentRuntimeIdentity::default() {
                return Ok(None);
            }
            return self.agent_error(id, "Cannot verify the terminal created by this launch. Deletion is suspended; inspect the launch outcome before retrying.".into()).map(|()| None);
        };
        let managed_name = identity::managed_name(agent.room_id, id);
        let kind = identity::provider_kind(agent.provider).label();
        let expected_session = identity.session_id.clone();
        let left_open = LeftOpenTerminal {
            agent_id: id,
            agent_name: agent.name.clone(),
            pane_id: pane.clone(),
            terminal_id: terminal.clone(),
        };
        let result = self.transport.request(Method::PaneCloseIfIdentity(
            schema::PaneCloseIfIdentityParams {
                pane_id: pane.clone(),
                expected_terminal_id: terminal.clone(),
                expected_agent: kind.into(),
                expected_managed_name: managed_name.clone(),
                expected_session_id: expected_session.clone(),
            },
        ));
        match result {
            Ok(ResponseResult::Ok {}) => Ok(None),
            Ok(other) => {
                if super::super::diagnostics::dev_enabled() {
                    tracing::debug!(event = "bus.deletion.close_unconfirmed", agent_id = id.0,
                        response = ?other, "Unexpected guarded terminal close response");
                }
                self.agent_error(id, "Terminal close could not be confirmed. The agent and messages were kept. Retry deletion after checking its terminal.".into()).map(|()| None)
            }
            Err(error) => {
                if super::super::diagnostics::dev_enabled() {
                    tracing::debug!(event = "bus.deletion.close_failed", agent_id = id.0,
                        code = ?error.code, detail = %error.message, "Guarded terminal close failed");
                }
                if error.code.as_deref() == Some("terminal_identity_changed") {
                    if self.reconcile_deleting_initial_session(id)? {
                        // The first session binding is now durable. Retry the exact
                        // same native guard; a different/rebound session still fails.
                        return self.stop_agent_terminal(id);
                    }
                    if self.native_ownership_released(&managed_name) {
                        // The native server cleared this launch's managed name, e.g.
                        // after the provider exited and the pane respawned a shell.
                        // Nothing Bus-owned remains to close and no retry can ever
                        // pass the guard; what runs in that pane now is not ours.
                        tracing::info!(event = "bus.deletion.terminal_released", agent_id = id.0,
                            pane_id = %left_open.pane_id, terminal_id = %left_open.terminal_id,
                            "Native terminal no longer carries the Bus managed name; left open");
                        return Ok(Some(left_open));
                    }
                    if expected_session.as_deref().is_some_and(|expected| {
                        self.owned_terminal_session_moved(&left_open, kind, &managed_name, expected)
                    }) {
                        // The provider session in this agent's own terminal
                        // moved on, as after an agent clear the server did not
                        // record or a chat resumed by hand. No retry can pass
                        // the guard, and the session there may not be Bus's to
                        // end, so the agent goes and the terminal stays open.
                        tracing::info!(event = "bus.deletion.session_moved", agent_id = id.0,
                            pane_id = %left_open.pane_id, terminal_id = %left_open.terminal_id,
                            "Owned terminal now runs another provider session; left open");
                        return Ok(Some(left_open));
                    }
                    if let Some(moved) = expected_session.as_deref().and_then(|expected| {
                        self.restored_terminal_session_moved(
                            &left_open,
                            kind,
                            &managed_name,
                            expected,
                        )
                    }) {
                        // After a restart the managed name moved to a restored
                        // terminal that kept another provider session, so Bus
                        // never rebound to it. Same reasoning as above.
                        tracing::info!(event = "bus.deletion.session_moved", agent_id = id.0,
                            pane_id = %moved.pane_id, terminal_id = %moved.terminal_id,
                            "Restored terminal runs another provider session; left open");
                        return Ok(Some(moved));
                    }
                }
                let message = if matches!(
                    error.code.as_deref(),
                    Some("unknown_method" | "invalid_request")
                ) {
                    "Update and restart the Bus server, then retry deletion. The agent and messages were kept.".into()
                } else if error.code.as_deref() == Some("terminal_identity_changed") {
                    "The terminal session changed. The agent and messages were kept. Check its terminal before retrying deletion.".into()
                } else {
                    "The terminal could not be closed. The agent and messages were kept. Check its terminal, then retry deletion.".into()
                };
                self.agent_error(id, message).map(|()| None)
            }
        }
    }

    /// Whether the native server still shows this agent's own terminal, pane,
    /// managed name and provider, with a provider session other than `expected`.
    /// A failed lookup proves nothing.
    fn owned_terminal_session_moved(
        &mut self,
        terminal: &LeftOpenTerminal,
        kind: &str,
        managed_name: &str,
        expected: &str,
    ) -> bool {
        let Ok(ResponseResult::AgentInfo { agent: info }) =
            self.transport
                .request(Method::AgentGet(schema::AgentTarget {
                    target: terminal.pane_id.clone(),
                }))
        else {
            return false;
        };
        info.terminal_id == terminal.terminal_id
            && info.pane_id == terminal.pane_id
            && info.name.as_deref() == Some(managed_name)
            && info.agent.as_deref() == Some(kind)
            && info.agent_session.is_some_and(|session| {
                session.source == format!("herdr:{kind}") && session.value != expected
            })
    }

    /// The one native terminal that carries this agent's managed name and
    /// provider when it is not the terminal Bus recorded and runs a provider
    /// session other than `expected`. A failed lookup proves nothing.
    fn restored_terminal_session_moved(
        &mut self,
        recorded: &LeftOpenTerminal,
        kind: &str,
        managed_name: &str,
        expected: &str,
    ) -> Option<LeftOpenTerminal> {
        let Ok(ResponseResult::AgentList { agents }) = self
            .transport
            .request(Method::AgentList(schema::EmptyParams {}))
        else {
            return None;
        };
        let mut named = agents
            .into_iter()
            .filter(|info| info.name.as_deref() == Some(managed_name));
        let info = named.next()?;
        if named.next().is_some()
            || info.terminal_id == recorded.terminal_id
            || info.agent.as_deref() != Some(kind)
            || !info.agent_session.as_ref().is_some_and(|session| {
                session.source == format!("herdr:{kind}") && session.value != expected
            })
        {
            return None;
        }
        Some(LeftOpenTerminal {
            pane_id: info.pane_id,
            terminal_id: info.terminal_id,
            ..recorded.clone()
        })
    }

    /// `agent.list` includes every terminal carrying a managed name, even with
    /// no detected provider, so absence is a native fact. A failed lookup
    /// proves nothing and keeps deletion suspended.
    fn native_ownership_released(&mut self, managed_name: &str) -> bool {
        matches!(
            self.transport.request(Method::AgentList(schema::EmptyParams {})),
            Ok(ResponseResult::AgentList { agents })
                if !agents.iter().any(|info| info.name.as_deref() == Some(managed_name))
        )
    }

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
        if let Some(room) = orchestrates {
            state
                .bind_orchestrator(id, room)
                .map_err(|e| e.to_string())?;
        }
        if !input.consent_project_hooks {
            if let Some(notice) =
                launch::setup_notice(identity::provider_kind(input.provider), &cwd)
            {
                let _ = events.send(BusEvent::SetupRequired {
                    input,
                    orchestrator,
                    notice,
                });
                return Ok(());
            }
        }
        let (mut prepared, hooks) = self.prepare_provider_launch(&input, id)?;
        if let Some(spec) = &orchestrator {
            self.prepare_orchestrator_prompt(&input, id, &mut state, spec, &mut prepared, &hooks)?;
        }
        let identity = self.persist_agent_launch(
            state,
            id,
            input.provider,
            hooks.launch_id,
            prepared.adopted_session.clone(),
        )?;
        let created = self
            .transport
            .request(Method::TabCreate(schema::TabCreateParams {
                workspace_id: None,
                cwd: Some(prepared.cwd.to_string_lossy().into_owned()),
                focus: false,
                label: Some(input.name.clone()),
                env: prepared.env,
            }));
        let (pane, terminal) = match created {
            Ok(ResponseResult::TabCreated { root_pane, .. }) => {
                (root_pane.pane_id, root_pane.terminal_id)
            }
            other => {
                return self.agent_error(
                    id,
                    format!("Tab creation outcome requires inspection; no retry. {other:?}"),
                );
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
        self.save(state)?;
        // This identity is durable before start. A missing response must not launch twice.
        let started = self
            .transport
            .request(Method::AgentStart(schema::AgentStartParams {
                name: identity::managed_name(input.room, id),
                kind: identity::provider_kind(input.provider).label().into(),
                pane_id: pane.clone(),
                args: prepared.args,
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
            other => self.agent_error(id,format!("Agent start outcome requires inspection of its owned terminal; no automatic retry or deletion. {other:?}")),
        }
    }

    /// Routing stays at the room boundary; providers only receive capture facts.
    fn prepare_provider_launch(
        &self,
        input: &AddAgent,
        id: AgentId,
    ) -> Result<(launch::PreparedLaunch, HookContext), String> {
        let binary = std::env::current_exe().map_err(|e| e.to_string())?;
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
        if provider == Provider::ClaudeCode {
            state.confirm_hook_setup(id).map_err(|e| e.to_string())?;
        }
        state
            .set_agent_error(
                id,
                Some(
                    match provider {
                        Provider::Codex | Provider::Cursor => {
                            "Not ready; waiting for the provider terminal."
                        }
                        Provider::ClaudeCode => "Not ready; waiting for provider hook readiness.",
                    }
                    .into(),
                ),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(identity)
    }

    fn report_adopted_session(
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

    fn prepare_orchestrator_prompt(
        &self,
        input: &AddAgent,
        id: AgentId,
        state: &mut super::BusState,
        spec: &OrchestratorSpec,
        prepared: &mut launch::PreparedLaunch,
        hooks: &HookContext,
    ) -> Result<(), String> {
        let values = orchestrator::PromptValues {
            room: state
                .room(spec.room)
                .map(|room| (room.name.clone(), room.id)),
            agent: state.agent(id).map(|a| a.name.clone()).unwrap_or_default(),
            docs: orchestrator::write_docs(&self.data_dir)?,
        };
        let template = spec
            .system_prompt
            .as_deref()
            .unwrap_or(orchestrator::DEFAULT_PROMPT);
        let text = orchestrator::fill(template, &values);
        let adopted = prepared.adopted_session.is_some();
        let path = orchestrator::write_prompt(&hooks.spool, &text)?;
        match orchestrator::prompt_args(input.provider, &path, adopted)? {
            Some(args) => prepared.args.extend(args),
            // Queued until the agent is ready, like any room message.
            None => {
                let master = input.room;
                state
                    .submit_message_with(
                        master,
                        Draft {
                            text: match input.provider {
                                Provider::Cursor => cursor::system_prompt::prompt_message(&text),
                                _ => orchestrator::prompt_message(&text),
                            },
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

    pub(super) fn agent_error(&mut self, id: AgentId, message: String) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .set_agent_error(id, Some(message.clone()))
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Err(message)
    }
}
