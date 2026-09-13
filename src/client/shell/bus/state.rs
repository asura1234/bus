use super::editor::Editor;
use super::{
    forms::{Form, Rename, Suggestions},
    render::View,
};
use crate::bus::{
    model::*,
    runtime::{BusCommand, BusEvent, BusHandle, BusSnapshot},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[cfg(test)]
#[path = "focus_tests.rs"]
mod focus_tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ComposerSize {
    #[default]
    Auto,
    Full,
    Compact,
}

#[derive(Clone, Debug)]
pub(super) struct LocalRoom {
    pub text: Editor,
    pub notes: Editor,
    pub recipients: BTreeSet<AgentId>,
    pub composer_size: ComposerSize,
    // None follows the caret; Some is an independently scrolled viewport.
    pub composer_scroll: Option<usize>,
    pub recall: Vec<String>,
    pub stash: Vec<String>,
    pub history_index: Option<usize>,
    pub live_draft: Option<String>,
    pub text_generation: u64,
    pub notes_generation: u64,
    pub recipient_generation: u64,
    pub saved_text_generation: u64,
    pub saved_notes_generation: u64,
    pub saved_recipient_generation: u64,
}
impl From<&Room> for LocalRoom {
    fn from(room: &Room) -> Self {
        Self {
            text: Editor::new(room.draft.text.clone()),
            notes: Editor::new(room.notes.clone()),
            recipients: room.draft.recipient_ids.clone(),
            composer_size: ComposerSize::Auto,
            composer_scroll: None,
            recall: Vec::new(),
            stash: Vec::new(),
            history_index: None,
            live_draft: None,
            text_generation: 0,
            notes_generation: 0,
            recipient_generation: 0,
            saved_text_generation: 0,
            saved_notes_generation: 0,
            saved_recipient_generation: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Effect {
    None,
    Text(RoomId, u64),
    Notes(RoomId, u64),
    Recipients(RoomId, u64),
    Files(RoomId),
    Submit(RoomId, u64),
    Shutdown,
}

#[derive(Clone, Debug)]
pub(super) struct Pending {
    pub id: u64,
    pub command: BusCommand,
    pub effect: Effect,
    pub enqueued: bool,
    pub result: Option<Result<(), String>>,
}

pub(in crate::client::shell) struct BusUi {
    pub handle: Option<BusHandle>,
    pub snapshot: Arc<BusSnapshot>,
    pub room: Option<RoomId>,
    pub(super) locals: BTreeMap<RoomId, LocalRoom>,
    pub(super) pending: VecDeque<Pending>,
    pub next_id: u64,
    pub send_intent: Option<RoomId>,
    pub error: Option<String>,
    pub(super) dismissed_snapshot_error: Option<String>,
    pub(super) failed: Vec<Pending>,
    pub(super) terminal: Option<AgentId>,
    pub(super) target_pane: Option<String>,
    pub(super) form: Option<Form>,
    pub(super) deletion: Option<super::deletion::DeleteDialog>,
    pub(super) rename: Option<Rename>,
    pub(super) notes_focus: bool,
    pub(super) recipient_menu: bool,
    pub(super) recipient_index: usize,
    pub(super) suggestions: Suggestions,
    pub(super) view: View,
    pub(super) main_scroll: usize,
    pub(super) history_follow_tail: bool,
    pub(super) history: super::history::History,
    /// Region receiving the current left-button drag, if it started a selection.
    pub(super) drag: Option<super::selection::Region>,
    /// History selection as (anchor, head); editor selections live in `Editor`.
    pub(super) history_selection: Option<(super::selection::Point, super::selection::Point)>,
    pub(super) recipient_scroll: u16,
    pub(super) sidebar_scroll: usize,
    pub(super) file_scroll: usize,
    pub(super) last_click: Option<(super::render::Action, std::time::Instant)>,
    pub(super) detail_path: Option<String>,
    pub(super) seed_first_room: bool,
    pub(super) quitting: Option<std::time::Instant>,
    pub(super) force_exit_available: bool,
    pub exit_ready: bool,
    pub(super) history_search: Option<HistorySearch>,
    pub(super) pending_line_continue: bool,
    pub(super) last_esc: Option<std::time::Instant>,
}

#[derive(Clone, Debug)]
pub(super) struct HistorySearch {
    pub query: String,
    pub selected: usize,
    pub live_draft: String,
}

impl BusUi {
    pub fn new(snapshot: Arc<BusSnapshot>) -> Self {
        let room = snapshot.state.rooms().next().map(|r| r.id);
        let locals = snapshot
            .state
            .rooms()
            .map(|r| (r.id, LocalRoom::from(r)))
            .collect();
        let seed_first_room = snapshot.state.is_pristine();
        Self {
            handle: None,
            snapshot,
            room,
            locals,
            pending: VecDeque::new(),
            next_id: 1,
            send_intent: None,
            error: None,
            dismissed_snapshot_error: None,
            failed: Vec::new(),
            terminal: None,
            target_pane: None,
            form: None,
            deletion: None,
            rename: None,
            notes_focus: false,
            recipient_menu: false,
            recipient_index: 0,
            suggestions: Suggestions::default(),
            view: View::default(),
            main_scroll: 0,
            history_follow_tail: true,
            history: super::history::History::default(),
            drag: None,
            history_selection: None,
            recipient_scroll: 0,
            sidebar_scroll: 0,
            file_scroll: 0,
            last_click: None,
            detail_path: None,
            seed_first_room,
            quitting: None,
            force_exit_available: false,
            exit_ready: false,
            history_search: None,
            pending_line_continue: false,
            last_esc: None,
        }
    }
    pub(super) fn queue(&mut self, command: BusCommand, effect: Effect) -> u64 {
        self.pending.retain(|p|p.enqueued || !matches!((&p.effect,&effect),
            (Effect::Text(a,_),Effect::Text(b,_)) | (Effect::Notes(a,_),Effect::Notes(b,_)) | (Effect::Recipients(a,_),Effect::Recipients(b,_)) if a==b));
        let id = self.next_id;
        self.next_id += 1;
        if let BusCommand::Submit(room) = &command {
            tracing::info!(
                event = "bus.message.submit",
                room_id = room.0,
                command_id = id,
                "Room submit queued for coordinator"
            );
        }
        self.pending.push_back(Pending {
            id,
            command,
            effect,
            enqueued: false,
            result: None,
        });
        id
    }
    pub fn text_changed(&mut self, room: RoomId) {
        if let Some(local) = self.locals.get_mut(&room) {
            local.composer_scroll = None;
            if local.text.text.is_empty() {
                local.composer_size = ComposerSize::Auto;
            }
            local.text_generation += 1;
            let (text, generation) = (local.text.text.clone(), local.text_generation);
            self.queue(
                BusCommand::SetDraftText(room, text),
                Effect::Text(room, generation),
            );
        }
    }
    pub(super) fn visible_error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| {
            self.snapshot
                .error
                .as_deref()
                .filter(|error| self.dismissed_snapshot_error.as_deref() != Some(*error))
        })
    }
    pub fn receive_event(&mut self, event: BusEvent) {
        match event {
            BusEvent::DevFocusRequested { room, agent } => {
                if let Some(agent) = agent {
                    // Show this agent's room in the sidebar without marking the
                    // room history read merely because its terminal was opened.
                    self.room = Some(room);
                    self.open_terminal(agent);
                } else {
                    self.open_room(room);
                }
            }
            BusEvent::CommandFinished { command_id, result } => {
                if let Some(pending) = self.pending.iter_mut().find(|p| p.id == command_id) {
                    if matches!(pending.command, BusCommand::Submit(_)) {
                        tracing::info!(
                            event = "bus.message.ack",
                            command_id,
                            outcome = if result.is_ok() { "queued" } else { "rejected" },
                            "Room received submit acknowledgement"
                        );
                    }
                    pending.result = Some(result);
                }
            }
            BusEvent::RoomCreated(room) => {
                self.open_room(room);
                if self.seed_first_room {
                    self.seed_first_room = false;
                    self.queue(BusCommand::SetNotes(room, "Goal: stay in the room to coordinate selected agents.\nNon-goals: automatic handoffs or permission approval.".into()), Effect::None);
                }
            }
            BusEvent::AgentAdded(agent) => {
                self.form = None;
                self.open_terminal(agent);
            }
            BusEvent::TerminalFocused { agent, pane_id } if self.terminal == Some(agent) => {
                self.target_pane = Some(pane_id);
            }
            BusEvent::Suggestions { query_id, result } if query_id == self.suggestions.query_id => {
                match result {
                    Ok(entries) => {
                        self.suggestions.entries = entries;
                        self.suggestions.selected = 0;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            BusEvent::SetupRequired { input, notice } => {
                self.suggestions.entries.clear();
                self.suggestions.query_id += 1;
                self.form = Some(Form::Consent { input, notice });
            }
            _ => {}
        }
    }
    pub fn receive_snapshot(&mut self, snapshot: Arc<BusSnapshot>) {
        crate::bus::diagnostics::replies(
            &self.snapshot.state,
            &snapshot.state,
            "bus.reply.received",
        );
        for room in snapshot.state.rooms() {
            self.locals
                .entry(room.id)
                .or_insert_with(|| LocalRoom::from(room));
            if let Some(local) = self.locals.get_mut(&room.id) {
                // Rebuilding unchanged notes would drop their caret and selection.
                if local.notes_generation == 0 && local.notes.text != room.notes {
                    local.notes = Editor::new(room.notes.clone());
                }
            }
        }
        let previous = std::mem::replace(&mut self.snapshot, snapshot);
        self.reconcile_deleted_targets(&previous.state);
    }
    pub fn settle(&mut self) {
        while self
            .pending
            .front()
            .is_some_and(|p| p.result.is_some() && self.snapshot.last_command_id >= p.id)
        {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            match pending.result.as_ref() {
                Some(Err(error)) => {
                    self.settle_deletion(pending.id, Some(error));
                    self.error = Some(error.clone());
                    self.send_intent = None;
                    if matches!(
                        pending.effect,
                        Effect::Text(..)
                            | Effect::Notes(..)
                            | Effect::Recipients(..)
                            | Effect::Files(..)
                    ) && super::deletion::target_exists(&pending.command, &self.snapshot.state)
                    {
                        self.failed.push(pending);
                    }
                }
                Some(Ok(())) => {
                    self.settle_deletion(pending.id, None);
                    match pending.effect {
                        Effect::Text(room, generation) => {
                            if let Some(local) = self.locals.get_mut(&room) {
                                local.saved_text_generation = generation;
                            }
                        }
                        Effect::Notes(room, generation) => {
                            if let Some(local) = self.locals.get_mut(&room) {
                                local.saved_notes_generation = generation;
                            }
                        }
                        Effect::Recipients(room, generation) => {
                            if let Some(local) = self.locals.get_mut(&room) {
                                local.saved_recipient_generation = generation;
                            }
                        }
                        Effect::Shutdown => self.exit_ready = true,
                        _ => {}
                    }
                    if let Effect::Submit(room, generation) = pending.effect {
                        if let Some(local) = self.locals.get_mut(&room) {
                            if local.text_generation == generation {
                                local.text = Editor::default();
                                local.composer_size = ComposerSize::Auto;
                                local.composer_scroll = None;
                            }
                        }
                    }
                    // Local editors remain authoritative; an old successful save never replaces
                    // a later generation. Only their exact command is retired here.
                }
                None => {}
            }
        }
        if self.pending.is_empty() {
            if let Some(room) = self.send_intent.take() {
                let generation = self
                    .locals
                    .get(&room)
                    .map_or(0, |local| local.text_generation);
                self.queue(BusCommand::Submit(room), Effect::Submit(room, generation));
            } else if self.quitting.is_some() && self.failed.is_empty() && !self.exit_ready {
                self.queue(BusCommand::Shutdown, Effect::Shutdown);
            }
        }
    }
    pub fn request_send(&mut self, room: RoomId) {
        if let Some(local) = self
            .locals
            .get_mut(&room)
            .filter(|local| local.text.text.trim() == "/help")
        {
            local.text = Editor::default();
            // A queued intent reads the latest saved draft. Consume this local
            // command before settling it; it must never become that payload.
            if self.send_intent == Some(room) {
                self.send_intent = None;
            }
            self.text_changed(room);
            self.form = Some(Form::Help { scroll: 0 });
            self.recipient_menu = false;
            self.detail_path = None;
            self.suggestions.entries.clear();
            self.suggestions.query_id += 1;
            return;
        }
        if self
            .locals
            .get(&room)
            .is_none_or(|local| local.recipients.is_empty())
        {
            self.error = Some("Choose agents with @ before sending.".into());
            tracing::info!(
                event = "bus.message.rejected",
                room_id = room.0,
                reason = "no_recipients",
                "Room send rejected locally"
            );
            return;
        }
        if self.send_intent.is_some()
            || self
                .pending
                .iter()
                .any(|p| matches!(p.effect, Effect::Submit(..)))
        {
            return;
        }
        let failed = std::mem::take(&mut self.failed);
        if let Some(local) = self.locals.get(&room) {
            tracing::debug!(event = "bus.message.requested", room_id = room.0,
                recipients = ?local.recipients, text_bytes = local.text.text.len(),
                "Room send requested");
        }
        for pending in failed {
            match pending.effect {
                Effect::Text(id, _) => self.text_changed(id),
                Effect::Recipients(id, _) => self.recipients_changed(id),
                Effect::Notes(id, _) => self.notes_changed(id),
                Effect::Files(id) if id != room => self.failed.push(pending),
                _ => {
                    self.queue(pending.command, pending.effect);
                }
            }
        }
        self.error = None;
        if let Some(local) = self.locals.get_mut(&room) {
            let text = local.text.text.clone();
            if !text.is_empty() && local.recall.last() != Some(&text) {
                local.recall.push(text);
            }
            local.history_index = None;
            local.live_draft = None;
        }
        self.send_intent = Some(room);
        self.history_follow_tail = true;
    }
    pub(super) fn apply_external_edit(&mut self, room: RoomId, text: String) {
        if let Some(local) = self.locals.get_mut(&room) {
            local.text = super::editor::Editor::new(text);
            local.history_index = None;
            local.live_draft = None;
        }
        self.text_changed(room);
    }
    pub(super) fn edit_in_external_editor(&mut self) -> Result<(), String> {
        let room = self.room.ok_or("No room selected")?;
        let text = self
            .locals
            .get(&room)
            .map(|local| local.text.text.clone())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("bus-prompt-{}.md", std::process::id()));
        std::fs::write(&path, text).map_err(|error| error.to_string())?;
        let argv =
            crate::platform::scrollback_editor_argv(&path).map_err(|error| error.to_string())?;
        let Some((program, args)) = argv.split_first() else {
            let _ = std::fs::remove_file(&path);
            return Err("editor command is empty".into());
        };
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let status = std::process::Command::new(program).args(args).status();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);
        let _ = crossterm::terminal::enable_raw_mode();
        let result = match status {
            Ok(status) if status.success() => std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .map(|edited| self.apply_external_edit(room, edited)),
            Ok(status) => Err(format!("editor exited with {status}")),
            Err(error) => Err(error.to_string()),
        };
        let _ = std::fs::remove_file(&path);
        result
    }
    pub fn notes_changed(&mut self, room: RoomId) {
        if let Some(local) = self.locals.get_mut(&room) {
            local.notes_generation += 1;
            let (text, generation) = (local.notes.text.clone(), local.notes_generation);
            self.queue(
                BusCommand::SetNotes(room, text),
                Effect::Notes(room, generation),
            );
        }
    }
    pub fn recipients_changed(&mut self, room: RoomId) {
        if let Some(local) = self.locals.get_mut(&room) {
            local.recipient_generation += 1;
            let (recipients, generation) = (local.recipients.clone(), local.recipient_generation);
            self.queue(
                BusCommand::SetRecipients(room, recipients),
                Effect::Recipients(room, generation),
            );
        }
    }
    /// Called only from client update handling. IO lives in the coordinator worker.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..256 {
            let Some(event) = self.handle.as_ref().and_then(BusHandle::try_event) else {
                break;
            };
            self.receive_event(event);
            changed = true;
        }
        if let Some(snapshot) = self.handle.as_ref().and_then(BusHandle::snapshot) {
            if snapshot.revision != self.snapshot.revision
                || snapshot.last_command_id != self.snapshot.last_command_id
                || snapshot.error != self.snapshot.error
            {
                self.receive_snapshot(snapshot);
                changed = true;
            }
        }
        self.settle();
        if self.quitting.is_some_and(|started| {
            started.elapsed() > std::time::Duration::from_secs(5) || !self.failed.is_empty()
        }) && !self.exit_ready
        {
            self.quitting = None;
            self.force_exit_available = true;
            self.error=Some("Unsaved changes or slow storage. Ctrl+C retries saving; Ctrl+Shift+Q exits and may lose unsaved edits.".into());
            changed = true;
        }
        // Confirmation must reach the worker even while an earlier Submit is
        // awaiting its post-poll snapshot. Preserve command order and exact
        // acknowledgements, but enqueue the prefix through deletion together.
        let dispatch_count = self
            .deletion
            .as_ref()
            .and_then(|dialog| dialog.command_id)
            .and_then(|id| self.pending.iter().position(|pending| pending.id == id))
            .map_or(1, |index| index + 1);
        for pending in self.pending.iter_mut().take(dispatch_count) {
            if !pending.enqueued {
                if let Some(handle) = &self.handle {
                    match handle.try_send(pending.id, pending.command.clone()) {
                        Ok(()) => {
                            pending.enqueued = true;
                            changed = true;
                        }
                        Err(error) => {
                            if self.error.as_ref() != Some(&error) {
                                tracing::warn!(
                                    event = "bus.command.enqueue_failed",
                                    command_id = pending.id,
                                    "Coordinator command channel unavailable; command retained"
                                );
                                self.error = Some(error);
                                changed = true;
                            }
                            break;
                        }
                    }
                }
            }
        }
        changed
    }
    /// Ctrl+C clears a visible room draft before it may quit Bus. A draft that
    /// is already being sent stays intact, but still keeps Bus open.
    pub(super) fn clear_composer(&mut self) -> bool {
        if self.quitting.is_some()
            || self.force_exit_available
            || self.deletion.is_some()
            || self.form.is_some()
            || self.rename.is_some()
            || self.terminal.is_some()
        {
            return false;
        }
        let Some(room) = self.room else {
            return false;
        };
        if self
            .locals
            .get(&room)
            .is_none_or(|local| local.text.text.is_empty())
        {
            return false;
        }
        let sending = self.send_intent == Some(room)
            || self
                .pending
                .iter()
                .any(|p| matches!(p.effect, Effect::Submit(id, _) if id == room));
        if !sending {
            if let Some(local) = self.locals.get_mut(&room) {
                local.text = Editor::default();
            }
            self.drag = None;
            self.text_changed(room);
        }
        true
    }
    pub(super) fn request_quit(&mut self) {
        self.quitting = Some(std::time::Instant::now());
        let failed = std::mem::take(&mut self.failed);
        for pending in failed {
            match pending.effect {
                Effect::Text(room, _) => self.text_changed(room),
                Effect::Notes(room, _) => self.notes_changed(room),
                Effect::Recipients(room, _) => self.recipients_changed(room),
                _ => {
                    self.queue(pending.command, pending.effect);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ui() -> (BusUi, RoomId) {
        let mut state = BusState::default();
        let room = state.create_room("bus").unwrap();
        let agent = state
            .create_agent(room, "author", Provider::Codex, "/project".into(), None)
            .unwrap();
        state.set_draft_recipients(room, [agent]).unwrap();
        (
            BusUi::new(Arc::new(BusSnapshot {
                state,
                revision: 0,
                last_command_id: 0,
                error: None,
            })),
            room,
        )
    }
    fn acknowledge(ui: &mut BusUi, id: u64, success: bool) {
        ui.receive_event(BusEvent::CommandFinished {
            command_id: id,
            result: if success {
                Ok(())
            } else {
                Err("disk full".into())
            },
        });
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = id;
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
    }
    #[test]
    fn delivery_logs_receipt_once_when_reply_snapshot_reaches_room() {
        let (mut ui, room) = ui();
        let agent = ui.snapshot.state.agents().next().unwrap().id;
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .set_draft_text(room, "PRIVATE_PROMPT")
            .unwrap();
        let request = snapshot.state.submit_draft(room, 1).unwrap()[0];
        // Project a completed reply from the coordinator, as the UI receives it.
        let mut value = serde_json::to_value(&snapshot.state).unwrap();
        value["rooms"][room.0.to_string()]["latest_replies"][agent.0.to_string()] = serde_json::json!({"request_id": request.0, "agent_id": agent.0,
                "text": "PRIVATE_REPLY", "received_at_ms": 2});
        snapshot.state = serde_json::from_value(value).unwrap();
        snapshot.revision += 1;
        let capture = crate::logging::test_capture::Capture::default();
        capture.run(|| {
            ui.receive_snapshot(Arc::new(snapshot.clone()));
            ui.receive_snapshot(Arc::new(snapshot));
        });
        let logs = capture.text();
        assert_eq!(logs.matches("bus.reply.received").count(), 1, "{logs}");
        assert!(
            logs.contains(&format!("request_id={}", request.0)),
            "{logs}"
        );
        assert!(!logs.contains("PRIVATE_"), "{logs}");
        assert_eq!(
            ui.snapshot.state.room(room).unwrap().latest_replies[&agent].text,
            "PRIVATE_REPLY"
        );
    }
    #[test]
    fn immediate_enter_waits_for_own_success_and_snapshot_before_submit() {
        let (mut ui, room) = ui();
        ui.locals.get_mut(&room).unwrap().text.insert("latest");
        ui.text_changed(room);
        ui.request_send(room);
        ui.receive_event(BusEvent::CommandFinished {
            command_id: 1,
            result: Ok(()),
        });
        ui.settle();
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::Submit(_))));
        acknowledge(&mut ui, 1, true);
        assert!(
            matches!(ui.pending.front().map(|p| &p.command), Some(BusCommand::Submit(r)) if *r == room)
        );
    }
    #[test]
    fn failed_save_cancels_send_and_later_success_never_acknowledges_it() {
        let (mut ui, room) = ui();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("newer than saved");
        ui.text_changed(room);
        ui.queue(BusCommand::LeaveRoom, Effect::None);
        ui.request_send(room);
        acknowledge(&mut ui, 1, false);
        acknowledge(&mut ui, 2, true);
        assert!(ui.send_intent.is_none());
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::Submit(_))));
        assert_eq!(ui.locals[&room].text.text, "newer than saved");
        assert!(ui.error.as_deref().is_some_and(|e| e.contains("disk full")));
    }
    #[test]
    fn old_submit_ack_preserves_newer_text_and_another_room_draft() {
        let (mut ui, room) = ui();
        ui.request_send(room);
        ui.settle();
        let id = ui.pending.front().expect("submit").id;
        ui.locals.get_mut(&room).unwrap().text.insert("next prompt");
        ui.text_changed(room);
        acknowledge(&mut ui, id, true);
        assert_eq!(ui.locals[&room].text.text, "next prompt");
    }
}
