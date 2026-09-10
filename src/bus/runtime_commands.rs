//! Worker commands, owned launch operations and terminal navigation.
use super::*;

impl Worker {
    pub(super) fn command(
        &mut self,
        command: BusCommand,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        let result = match command {
            BusCommand::CreateRoom(name) => {
                let id = state.create_room(&name).map_err(|e| e.to_string())?;
                self.save(state)?;
                let _ = events.send(BusEvent::RoomCreated(id));
                return Ok(());
            }
            BusCommand::RenameRoom(id, name) => state.rename_room(id, &name),
            BusCommand::RenameAgent(id, name) => state.rename_agent(id, &name),
            BusCommand::SelectRoom(id) => state.select_room(id),
            BusCommand::LeaveRoom => {
                state.leave_room_view();
                Ok(())
            }
            BusCommand::MarkRoomSeen(id) => state.mark_room_seen(id),
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
            BusCommand::Submit(room) => state
                .submit_draft(room, crate::bus::io::now_ms())
                .map(|_| ()),
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
            BusCommand::AddAgent(input) => return self.add_agent(input, events),
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
            BusCommand::Shutdown => return Ok(()),
        };
        result.map_err(|e| e.to_string())?;
        self.save(state)
    }

    fn add_agent(
        &mut self,
        input: AddAgent,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let cwd = launch::canonical_directory(&input.cwd)?;
        if !input.consent_project_hooks {
            if let Some(notice) = launch::setup_notice(input.provider, &cwd) {
                let _ = events.send(BusEvent::SetupRequired { input, notice });
                return Ok(());
            }
        }
        let mut state = self.state.clone();
        let id = state
            .create_agent(input.room, &input.name, input.provider, cwd, None)
            .map_err(|e| e.to_string())?;
        let prepared = launch::prepare(
            &input,
            id,
            &self.data_dir,
            &std::env::current_exe().map_err(|e| e.to_string())?,
        )?;
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
                Some("Launching; awaiting provider hook readiness".into()),
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
