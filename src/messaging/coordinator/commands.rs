//! Single-writer command dispatch.
use super::{
    mpsc, schema, AgentId, BusCommand as C, BusEvent, BusState, Method, Provider, ResponseResult,
    RoomId, Worker,
};
use crate::messaging::storage::state_store::JournalOp;

impl Worker {
    pub(super) fn command(
        &mut self,
        command: C,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        let queued = matches!(command, C::SubmitQueued(_));
        let result = match command {
            C::CreateRoom(name) => return self.persist_new_room(state, &name, events),
            C::RenameRoom(id, name) => state.rename_room(id, &name),
            C::RenameAgent(id, name) => state.rename_agent(id, &name),
            C::DeleteRoom(id) => return self.delete_room(id, events),
            C::DeleteAgent(id) => return self.delete_agent(id, events),
            C::SelectRoom(id) => state.select_room(id),
            C::LeaveRoom => {
                state.leave_room_view();
                Ok(())
            }
            C::MarkRoomSeen(id) => state.mark_room_seen(id),
            C::SetRoomSound(id, on) => {
                state.set_room_sound(id, on).map_err(|e| e.to_string())?;
                self.record_master_sound(&state, id)?;
                Ok(())
            }
            C::SetRoomSoundName(id, name) => {
                state
                    .set_room_sound_name(id, name)
                    .map_err(|e| e.to_string())?;
                self.record_master_sound(&state, id)?;
                Ok(())
            }
            C::SetAllRoomsSound(on) => {
                self.set_all_rooms_sound(Some(on), None, events)?;
                return Ok(());
            }
            C::SetAllRoomsSoundName(name) => {
                self.set_all_rooms_sound(None, Some(name), events)?;
                return Ok(());
            }
            C::SetMaxCompactionsPerAgent(limit) => {
                return self.set_max_compactions_per_agent(limit, events);
            }
            C::SetIdleAgentsAlert(limit) => return self.set_idle_agents_alert(limit, events),
            C::SetDiskWarningGb(value) => return self.set_disk_threshold(true, value, events),
            C::SetDiskCriticalGb(value) => return self.set_disk_threshold(false, value, events),
            C::SetFeishuRecipient(recipient) => {
                return self.set_feishu_recipient(recipient, events);
            }
            C::SetFeishuCliPath(path) => return self.set_feishu_cli_path(path, events),
            C::SetSidebarWidth(width) => return self.set_sidebar_width(width, events),
            C::SetFeishuNotifications(on) => return self.set_feishu_notifications(on, events),
            C::SetDeveloperSoundName(name) => return self.set_developer_sound(name, events),
            C::SetNotes(id, text) => state.set_room_notes(id, &text),
            C::SetDraftText(id, text) => {
                state.set_draft_text(id, &text).map_err(|e| e.to_string())?;
                return self.journal(state, JournalOp::DraftText { room: id, text });
            }
            C::SetRecipients(id, recipients) => state.set_draft_recipients(id, recipients),
            C::AttachFile(room, path) => {
                let home = std::env::home_dir().ok_or("Home directory unavailable")?;
                let path = crate::messaging::attachments::validate_attachment(&path, &home)
                    .map_err(|e| e.to_string())?;
                state.attach_file(room, path)
            }
            C::RemoveFile(room, path) => state.remove_file(room, &path),
            C::Submit(room) | C::SubmitQueued(room) => {
                return self.persist_draft_submission(state, room, queued);
            }
            C::SetDetails(id, expanded) => state.set_agent_details_disclosed(id, expanded),
            C::CompleteHookSetup(id) => {
                complete_hook_setup(&mut state, id)?;
                Ok(())
            }
            C::Suggestions {
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
            C::AddAgent(input) => return self.add_agent(input, None, events),
            C::AddOrchestrator(input, spec) => return self.add_agent(input, Some(spec), events),
            C::FocusTerminal(id) => {
                self.focus_terminal(&mut state, id, events)?;
                Ok(())
            }
            C::Control(_) | C::Shutdown => return Ok(()),
        };
        result.map_err(|e| e.to_string())?;
        self.save(state)
    }

    fn persist_new_room(
        &mut self,
        mut state: BusState,
        name: &str,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let id = state.create_room(name).map_err(|e| e.to_string())?;
        self.apply_new_room_sound(&mut state, id);
        self.save(state)?;
        let _ = events.send(BusEvent::RoomCreated(id));
        Ok(())
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
        self.journal(
            state,
            JournalOp::Submit {
                room,
                queued,
                now_ms: now,
            },
        )?;
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
