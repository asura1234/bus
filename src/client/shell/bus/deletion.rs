//! Client-only confirmation; the coordinator owns persisted deletion and stopping sessions.
use super::{render::Action, *};
use crate::{
    bus::{model::*, runtime::BusCommand},
    raw_input::RawInputEvent,
};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeleteTarget {
    Room(RoomId),
    Agent(AgentId),
}

pub(super) struct DeleteDialog {
    pub target: DeleteTarget,
    pub command_id: Option<u64>,
    pub error: Option<String>,
}

impl BusUi {
    pub(super) fn start_delete(&mut self, target: DeleteTarget) {
        let exists = match target {
            DeleteTarget::Room(id) => self.snapshot.state.room(id).is_some(),
            DeleteTarget::Agent(id) => self.snapshot.state.agent(id).is_some(),
        };
        if exists && self.deletion.is_none() {
            self.deletion = Some(DeleteDialog {
                target,
                command_id: None,
                error: None,
            });
            self.last_click = None;
        }
    }

    pub(super) fn cancel_delete(&mut self) {
        if self
            .deletion
            .as_ref()
            .is_some_and(|dialog| dialog.command_id.is_none())
        {
            self.deletion = None;
            self.last_click = None;
        }
    }

    pub(super) fn confirm_delete(&mut self) {
        let Some(dialog) = self
            .deletion
            .as_ref()
            .filter(|dialog| dialog.command_id.is_none())
        else {
            return;
        };
        if dialog.error.is_some() {
            self.cancel_delete();
            return;
        }
        let target = dialog.target;
        let room = match target {
            DeleteTarget::Room(id) => Some(id),
            DeleteTarget::Agent(id) => self.snapshot.state.agent(id).map(|agent| agent.room_id),
        };
        if self.send_intent == room {
            self.send_intent = None;
        }
        // Once confirmed, a not-yet-dispatched room prompt must not outrun deletion.
        self.pending.retain(|pending| {
            pending.enqueued
                || !matches!(pending.command, BusCommand::Submit(id) if Some(id) == room)
        });
        let command = match target {
            DeleteTarget::Room(id) => BusCommand::DeleteRoom(id),
            DeleteTarget::Agent(id) => BusCommand::DeleteAgent(id),
        };
        let id = self.queue(command, Effect::None);
        if let Some(dialog) = &mut self.deletion {
            dialog.command_id = Some(id);
            dialog.error = None;
        }
    }

    pub(super) fn deletion_input(&mut self, event: &RawInputEvent) -> bool {
        if self.deletion.is_none() {
            return false;
        }
        match event {
            RawInputEvent::Key(key) => {
                if key.kind == KeyEventKind::Press && key.modifiers == KeyModifiers::NONE {
                    match key.code {
                        KeyCode::Esc => self.cancel_delete(),
                        KeyCode::Enter => self.confirm_delete(),
                        _ => {}
                    }
                }
            }
            RawInputEvent::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    let action = self
                        .view
                        .hits
                        .iter()
                        .rev()
                        .find(|hit| hit.rect.contains((mouse.column, mouse.row).into()))
                        .map(|hit| &hit.action);
                    match action {
                        Some(Action::CancelDelete) => self.cancel_delete(),
                        Some(Action::ConfirmDelete) => self.confirm_delete(),
                        _ => {}
                    }
                }
            }
            RawInputEvent::Paste(_) | RawInputEvent::Text(_) => {}
            _ => return false,
        }
        true
    }

    pub(super) fn settle_deletion(&mut self, id: u64, error: Option<&String>) {
        if self
            .deletion
            .as_ref()
            .is_some_and(|dialog| dialog.command_id == Some(id))
        {
            if let Some(error) = error {
                if let Some(dialog) = &mut self.deletion {
                    dialog.command_id = None;
                    dialog.error = Some(error.clone());
                }
            } else {
                self.deletion = None;
                self.error = None;
                self.last_click = None;
            }
        }
    }

    pub(super) fn reconcile_deleted_targets(&mut self, previous: &BusState) {
        let state = &self.snapshot.state;
        // Creation events can arrive before their snapshot. Only a target that
        // actually disappeared from known state is deleted, not a new target
        // that this intermediate snapshot has not observed yet.
        let room_removed = |id| previous.room(id).is_some() && state.room(id).is_none();
        let agent_removed = |id| previous.agent(id).is_some() && state.agent(id).is_none();
        self.locals.retain(|id, _| !room_removed(*id));
        for local in self.locals.values_mut() {
            local.recipients.retain(|id| !agent_removed(*id));
        }
        // Retire stale local retries, but keep in-flight commands until their exact ack.
        let target_removed = |command: &BusCommand| {
            target_exists(command, previous) && !target_exists(command, state)
        };
        self.pending
            .retain(|pending| pending.enqueued || !target_removed(&pending.command));
        self.failed
            .retain(|pending| !target_removed(&pending.command));
        if self.send_intent.is_some_and(room_removed) {
            self.send_intent = None;
        }
        let missing_room = self.room.is_some_and(room_removed);
        let missing_terminal = self.terminal.is_some_and(agent_removed);
        if missing_room || missing_terminal {
            self.terminal = None;
            self.target_pane = None;
            self.form = None;
            self.rename = None;
            self.recipient_menu = false;
            self.notes_focus = false;
            self.main_scroll = 0;
            self.history_follow_tail = true;
            self.recipient_scroll = 0;
            if missing_room {
                self.room = state.rooms().next().map(|room| room.id);
            }
            if let Some(room) = self.room {
                self.queue(BusCommand::SelectRoom(room), Effect::None);
            }
        }
    }
}

pub(super) fn target_exists(command: &BusCommand, state: &BusState) -> bool {
    match command {
        BusCommand::RenameRoom(id, _)
        | BusCommand::SelectRoom(id)
        | BusCommand::MarkRoomSeen(id)
        | BusCommand::SetNotes(id, _)
        | BusCommand::SetDraftText(id, _)
        | BusCommand::SetRecipients(id, _)
        | BusCommand::Quote(id, _)
        | BusCommand::AttachFile(id, _)
        | BusCommand::RemoveFile(id, _)
        | BusCommand::Submit(id) => state.room(*id).is_some(),
        BusCommand::RenameAgent(id, _)
        | BusCommand::SetDetails(id, _)
        | BusCommand::FocusTerminal(id)
        | BusCommand::CompleteHookSetup(id) => state.agent(*id).is_some(),
        BusCommand::AddAgent(input) => state.room(input.room).is_some(),
        // Deletion is allowed to acknowledge a now-absent target idempotently.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::runtime::BusSnapshot;
    use std::sync::Arc;

    fn deletion_ui() -> (BusUi, RoomId) {
        let mut state = BusState::default();
        let room = state.create_room("room").unwrap();
        let mut ui = BusUi::new(Arc::new(BusSnapshot {
            state,
            revision: 0,
            last_command_id: 0,
            error: None,
        }));
        ui.start_delete(DeleteTarget::Room(room));
        (ui, room)
    }

    #[test]
    fn confirming_delete_failure_dismisses_without_retry_or_draft_mutation() {
        let (mut ui, room) = deletion_ui();
        ui.confirm_delete();
        let command = ui.deletion.as_ref().unwrap().command_id.unwrap();
        ui.settle_deletion(command, Some(&"Close failed".into()));
        ui.send_intent = Some(room);
        ui.queue(BusCommand::Submit(room), Effect::None);
        let pending = ui.pending.len();

        ui.confirm_delete();

        assert!(
            ui.deletion.is_none(),
            "acknowledging an error must dismiss it"
        );
        assert_eq!(
            ui.pending.len(),
            pending,
            "must not enqueue another deletion"
        );
        assert_eq!(
            ui.send_intent,
            Some(room),
            "dismissal must preserve new input"
        );
    }

    #[test]
    fn deletion_in_flight_ignores_confirm_and_cancel() {
        let (mut ui, _) = deletion_ui();
        ui.confirm_delete();
        let command = ui.deletion.as_ref().unwrap().command_id;
        let pending = ui.pending.len();
        ui.confirm_delete();
        ui.cancel_delete();
        assert_eq!(ui.deletion.as_ref().unwrap().command_id, command);
        assert_eq!(ui.pending.len(), pending);
    }

    #[test]
    fn cancelling_delete_confirmation_preserves_room_without_command() {
        let (mut ui, room) = deletion_ui();
        ui.cancel_delete();
        assert!(ui.deletion.is_none());
        assert!(ui.pending.is_empty());
        assert!(ui.snapshot.state.room(room).is_some());
    }
}
