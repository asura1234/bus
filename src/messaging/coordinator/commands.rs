//! Single-writer command dispatch.
use super::{
    mpsc, schema, AgentId, BusCommand, BusEvent, BusState, Method, Provider, ResponseResult,
    RoomId, Worker,
};

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
                self.apply_new_room_sound(&mut state, id);
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
            BusCommand::SetRoomSound(id, on) => {
                state.set_room_sound(id, on).map_err(|e| e.to_string())?;
                self.record_master_sound(&state, id)?;
                Ok(())
            }
            BusCommand::SetRoomSoundName(id, name) => {
                state
                    .set_room_sound_name(id, name)
                    .map_err(|e| e.to_string())?;
                self.record_master_sound(&state, id)?;
                Ok(())
            }
            BusCommand::SetAllRoomsSound(on) => {
                self.set_all_rooms_sound(Some(on), None, events)?;
                return Ok(());
            }
            BusCommand::SetAllRoomsSoundName(name) => {
                self.set_all_rooms_sound(None, Some(name), events)?;
                return Ok(());
            }
            BusCommand::SetMaxCompactionsPerAgent(limit) => {
                return self.set_max_compactions_per_agent(limit, events);
            }
            BusCommand::SetNotes(id, text) => state.set_room_notes(id, &text),
            BusCommand::SetDraftText(id, text) => state.set_draft_text(id, &text),
            BusCommand::SetRecipients(id, recipients) => state.set_draft_recipients(id, recipients),
            BusCommand::AttachFile(room, path) => {
                let home = std::env::home_dir().ok_or("Home directory unavailable")?;
                let path = crate::messaging::attachments::validate_attachment(&path, &home)
                    .map_err(|e| e.to_string())?;
                state.attach_file(room, path)
            }
            BusCommand::RemoveFile(room, path) => state.remove_file(room, &path),
            BusCommand::Submit(room) | BusCommand::SubmitQueued(room) => {
                return self.persist_draft_submission(state, room, queued);
            }
            BusCommand::SetDetails(id, expanded) => state.set_agent_details_disclosed(id, expanded),
            BusCommand::CompleteHookSetup(id) => {
                complete_hook_setup(&mut state, id)?;
                Ok(())
            }
            BusCommand::Suggestions {
                query_id,
                input,
                directories_only,
            } => {
                let _ = events.send(BusEvent::Suggestions {
                    query_id,
                    result: crate::agents::providers::suggest::suggestions(
                        &input,
                        directories_only,
                    ),
                });
                return Ok(());
            }
            BusCommand::AddAgent(input) => return self.add_agent(input, None, events),
            BusCommand::AddOrchestrator(input, spec) => {
                return self.add_agent(input, Some(spec), events)
            }
            BusCommand::FocusTerminal(id) => {
                self.focus_terminal(&mut state, id, events)?;
                Ok(())
            }
            BusCommand::Dev(_) | BusCommand::Shutdown => return Ok(()),
        };
        result.map_err(|e| e.to_string())?;
        self.save(state)
    }

    fn persist_draft_submission(
        &mut self,
        mut state: BusState,
        room: RoomId,
        queued: bool,
    ) -> Result<(), String> {
        let now = crate::messaging::storage::io::now_ms();
        let requests = if queued {
            state.submit_draft_queued(room, now)
        } else {
            state.submit_draft(room, now)
        }
        .map_err(|e| e.to_string())?;
        self.save(state)?;
        for id in requests {
            super::super::diagnostics::request(&self.state, id, "bus.message.queued", "persisted");
        }
        Ok(())
    }

    fn focus_terminal(
        &mut self,
        state: &mut BusState,
        id: AgentId,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
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
}

fn complete_hook_setup(state: &mut BusState, id: AgentId) -> Result<(), String> {
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
    }.map_err(|e| e.to_string())
}
