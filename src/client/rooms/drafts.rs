//! Per-room drafts, generation tracking and send/edit intent.
use super::{editor::Editor, forms::Form, BusUi, Effect};
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentRecipients, Room, RoomId},
};

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
    pub recipients: AgentRecipients,
    pub composer_size: ComposerSize,
    // None follows the caret; Some is an independently scrolled viewport.
    pub composer_scroll: Option<usize>,
    // The notes box scrolls the same way: None follows the caret.
    pub notes_scroll: Option<usize>,
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
            notes_scroll: None,
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

impl BusUi {
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

    /// Sends like Enter, but the message waits for each agent's own turn.
    pub fn request_queued_send(&mut self, room: RoomId) {
        self.request_send(room);
        self.send_queued = self.send_intent == Some(room);
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
        // An Enter on an empty composer (often a second Enter after a send)
        // has nothing to send. Submitting it would only come back as "The
        // message has no text or files" and sit over the next typed message.
        if !self.draft_has_content(room) {
            tracing::debug!(
                event = "bus.message.ignored",
                room_id = room.0,
                reason = "empty_draft",
                "Room send with an empty draft ignored"
            );
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
        self.send_requested_at = Some(std::time::Instant::now());
        // Each send picks its own delivery; a cancelled Option+Enter must not
        // turn a later plain Enter into a queued send.
        self.send_queued = false;
        self.history_follow_tail = true;
    }

    /// The local text, or a file the coordinator holds or is still attaching.
    fn draft_has_content(&self, room: RoomId) -> bool {
        self.locals
            .get(&room)
            .is_some_and(|local| !local.text.text.is_empty())
            || self
                .snapshot
                .state
                .room(room)
                .is_some_and(|room| !room.draft.files.is_empty())
            || self
                .pending
                .iter()
                .chain(&self.failed)
                .any(|p| matches!(&p.command, BusCommand::AttachFile(id, _) if *id == room))
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
            // Editing brings the caret back into view.
            local.notes_scroll = None;
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

    /// Ctrl+C clears a visible room draft; it never quits Bus. A draft that is
    /// already being sent stays intact.
    pub(super) fn clear_composer(&mut self) -> bool {
        if self.quitting.is_some()
            || self.force_exit_available
            || self.deletion.is_some()
            || self.alert.is_some()
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
}
