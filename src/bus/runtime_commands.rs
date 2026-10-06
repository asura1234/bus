//! Worker commands, owned launch operations and terminal navigation.
use super::*;
use crate::bus::orchestrator::{self, OrchestratorSpec};

impl Worker {
    pub(super) fn command(
        &mut self,
        command: BusCommand,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        let queued = matches!(command, BusCommand::SubmitQueued(_));
        let result = match command {
            BusCommand::CreateRoom(name) => {
                let id = state.create_room(&name).map_err(|e| e.to_string())?;
                self.save(state)?;
                let _ = events.send(BusEvent::RoomCreated(id));
                return Ok(());
            }
            BusCommand::RenameRoom(id, name) => state.rename_room(id, &name),
            BusCommand::RenameAgent(id, name) => state.rename_agent(id, &name),
            BusCommand::DeleteRoom(id) => return self.delete_room(id, events),
            BusCommand::DeleteAgent(id) => return self.delete_agent(id, events),
            BusCommand::SelectRoom(id) => state.select_room(id),
            BusCommand::LeaveRoom => {
                state.leave_room_view();
                Ok(())
            }
            BusCommand::MarkRoomSeen(id) => state.mark_room_seen(id),
            BusCommand::SetRoomSound(id, on) => state.set_room_sound(id, on),
            BusCommand::SetRoomSoundName(id, name) => state.set_room_sound_name(id, name),
            BusCommand::SetNotes(id, text) => state.set_room_notes(id, &text),
            BusCommand::SetDraftText(id, text) => state.set_draft_text(id, &text),
            BusCommand::SetRecipients(id, recipients) => state.set_draft_recipients(id, recipients),
            BusCommand::Quote(room, agent) => {
                let text = state
                    .room(room)
                    .and_then(|room| room.latest_replies.get(&agent))
                    .ok_or("No reply to quote")?
                    .text
                    .clone();
                state.quote_reply(room, agent, &text)
            }
            BusCommand::AttachFile(room, path) => {
                let home = std::env::home_dir().ok_or("Home directory unavailable")?;
                let path = crate::bus::files::validate_attachment(&path, &home)
                    .map_err(|e| e.to_string())?;
                state.attach_file(room, path)
            }
            BusCommand::RemoveFile(room, path) => state.remove_file(room, &path),
            BusCommand::Submit(room) | BusCommand::SubmitQueued(room) => {
                let now = crate::bus::io::now_ms();
                let requests = if queued {
                    state.submit_draft_queued(room, now)
                } else {
                    state.submit_draft(room, now)
                }
                .map_err(|e| e.to_string())?;
                self.save(state)?;
                for id in requests {
                    super::super::diagnostics::request(
                        &self.state,
                        id,
                        "bus.message.queued",
                        "persisted",
                    );
                }
                return Ok(());
            }
            BusCommand::SetDetails(id, expanded) => state.set_agent_details_disclosed(id, expanded),
            BusCommand::CompleteHookSetup(id) => {
                if state
                    .agent(id)
                    .is_some_and(|agent| agent.session_binding_invalidated)
                {
                    return Err("This provider session changed; create a new Bus agent. Existing requests remain preserved.".into());
                }
                state.confirm_hook_setup(id).map_err(|e| e.to_string())?;
                if state.agent(id).is_some_and(|a| {
                    (a.runtime_identity.session_id.is_some() || a.provider == Provider::Codex)
                        && a.current_request.is_none()
                }) {
                    state.set_agent_error(id, None)
                } else {
                    state.set_agent_error(id,Some("Awaiting matching provider session-start hook. Trust all Bus hooks, then restart/resume normally.".into()))
                }
            }
            BusCommand::Suggestions {
                query_id,
                input,
                directories_only,
            } => {
                let _ = events.send(BusEvent::Suggestions {
                    query_id,
                    result: launch::suggestions(&input, directories_only),
                });
                return Ok(());
            }
            BusCommand::AddAgent(input) => return self.add_agent(input, None, events),
            BusCommand::AddOrchestrator(input, spec) => {
                return self.add_agent(input, Some(spec), events)
            }
            BusCommand::SetOrchestrates(agent, room) => {
                let previous = state.agent(agent).and_then(|a| a.orchestrates);
                state
                    .set_agent_orchestrates(agent, room)
                    .map_err(|e| e.to_string())?;
                // The system prompt is fixed at launch, so tell the orchestrator.
                if previous != room {
                    let name = state
                        .agent(agent)
                        .map(|a| a.name.clone())
                        .unwrap_or_default();
                    let target = room.and_then(|room| state.room(room));
                    let text = orchestrator::reassigned_message(
                        &name,
                        target.map(|room| (room.name.as_str(), room.id)),
                    );
                    let master = state
                        .agent(agent)
                        .map(|a| a.room_id)
                        .ok_or("Unknown agent")?;
                    state
                        .submit_message_from(
                            master,
                            Draft {
                                text,
                                files: Vec::new(),
                                recipient_ids: [agent].into_iter().collect(),
                            },
                            Author::Human,
                            crate::bus::io::now_ms(),
                        )
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            }
            BusCommand::FocusTerminal(id) => {
                let agent = state.agent(id).ok_or("Unknown agent")?;
                let target = agent
                    .runtime_identity
                    .pane_id
                    .clone()
                    .ok_or("Agent has no terminal; inspect its launch error")?;
                let result = self
                    .transport
                    .request(Method::PaneFocus(schema::PaneTarget { pane_id: target }))
                    .map_err(|e| e.message)?;
                let ResponseResult::PaneInfo { pane } = result else {
                    return Err("Unexpected focus response".into());
                };
                let _ = events.send(BusEvent::TerminalFocused {
                    agent: id,
                    pane_id: pane.pane_id,
                });
                state.leave_room_view();
                Ok(())
            }
            BusCommand::Dev(_) | BusCommand::Shutdown => return Ok(()),
        };
        result.map_err(|e| e.to_string())?;
        self.save(state)
    }

    fn delete_agent(&mut self, id: AgentId, events: &mpsc::Sender<BusEvent>) -> Result<(), String> {
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

    fn delete_room(&mut self, id: RoomId, events: &mpsc::Sender<BusEvent>) -> Result<(), String> {
        let mut state = self.state.clone();
        state.prepare_delete_room(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        let agents: Vec<_> = self
            .state
            .agents()
            .filter(|agent| agent.room_id == id)
            .map(|agent| agent.id)
            .collect();
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
        let managed_name = format!("bus-r{}-a{}", agent.room_id.0, id.0);
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
                expected_agent: launch::provider_kind(agent.provider).into(),
                expected_managed_name: managed_name.clone(),
                expected_session_id: identity.session_id.clone(),
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

    fn add_agent(
        &mut self,
        input: AddAgent,
        orchestrator: Option<OrchestratorSpec>,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let orchestrates = orchestrator.as_ref().and_then(|spec| spec.room);
        let cwd = launch::canonical_directory(&input.cwd)?;
        // One provider session belongs to one Bus agent: bound by its hook, or
        // reserved by a launch that adopted it and has not reported yet.
        if let Some(session) = launch::adopted_session(input.provider, &input.extra_args)? {
            if let Some(owner) = self.state.agents().find(|agent| {
                let identity = &agent.runtime_identity;
                identity.session_id.as_deref() == Some(session.as_str())
                    || identity.launch_id.as_ref().is_some_and(|launch| {
                        launch::reserved_session(&self.data_dir.join("callbacks").join(launch))
                            .is_some_and(|reserved| reserved == session)
                    })
            }) {
                return Err(format!(
                    "Session {session} already belongs to Bus agent {}",
                    owner.name
                ));
            }
        }
        let mut state = self.state.clone();
        let id = state
            .create_agent(input.room, &input.name, input.provider, cwd.clone(), None)
            .map_err(|e| e.to_string())?;
        // Validate the assignment before any consent prompt or launch side effect.
        if orchestrator.is_some() {
            state
                .set_agent_orchestrates(id, orchestrates)
                .map_err(|e| e.to_string())?;
        }
        if !input.consent_project_hooks {
            if let Some(notice) = launch::setup_notice(input.provider, &cwd) {
                let _ = events.send(BusEvent::SetupRequired {
                    input,
                    orchestrator,
                    notice,
                });
                return Ok(());
            }
        }
        let prepared = launch::prepare(
            &input,
            id,
            &self.data_dir,
            &std::env::current_exe().map_err(|e| e.to_string())?,
        )?;
        let mut prepared = prepared;
        let spool = self
            .data_dir
            .join("callbacks")
            .join(&prepared.manifest.launch_id);
        if let Some(spec) = &orchestrator {
            let values = orchestrator::PromptValues {
                room: orchestrates
                    .and_then(|room| state.room(room))
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
            let path = orchestrator::write_prompt(&spool, &text)?;
            match orchestrator::prompt_args(input.provider, &path, adopted)? {
                Some(args) => prepared.args.extend(args),
                // Queued until the agent is ready, like any room message.
                None => {
                    let master = input.room;
                    state
                        .submit_message_with(
                            master,
                            Draft {
                                text: orchestrator::prompt_message(&text),
                                files: Vec::new(),
                                recipient_ids: [id].into_iter().collect(),
                            },
                            Author::Human,
                            crate::bus::io::now_ms(),
                            // The system prompt is the agent's first turn of its own.
                            true,
                        )
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        let identity = AgentRuntimeIdentity {
            launch_id: Some(prepared.manifest.launch_id),
            ..Default::default()
        };
        state
            .set_agent_runtime_identity(id, identity.clone())
            .map_err(|e| e.to_string())?;
        if input.provider == Provider::ClaudeCode {
            state.confirm_hook_setup(id).map_err(|e| e.to_string())?;
        }
        state
            .set_agent_error(
                id,
                Some(
                    match input.provider {
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
                    ..identity
                },
            )
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        // This identity is durable before start. A missing response must not launch twice.
        let started = self
            .transport
            .request(Method::AgentStart(schema::AgentStartParams {
                name: format!("bus-r{}-a{}", input.room.0, id.0),
                kind: launch::provider_kind(input.provider).into(),
                pane_id: pane,
                args: prepared.args,
                timeout_ms: None,
            }));
        match started {
            Ok(ResponseResult::AgentStarted {..}) => { let _ = events.send(BusEvent::AgentAdded(id)); Ok(()) },
            other => self.agent_error(id,format!("Agent start outcome requires inspection of its owned terminal; no automatic retry or deletion. {other:?}")),
        }
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
