//! Per-room drafts, generation tracking and send/edit intent.
use super::{editor::Editor, forms::Form, BusUi, Effect};
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentRecipients, Draft, Room, RoomId},
};

pub(super) fn attachment_path(input: &str) -> std::path::PathBuf {
    let home = std::env::home_dir();
    if let Some(home) = &home {
        if let Ok(path) = crate::messaging::attachments::validate_attachment(input, home) {
            return path;
        }
    }
    if input == "~" {
        return home.unwrap_or_else(|| std::path::PathBuf::from(input));
    }
    if let Some(remainder) = input.strip_prefix("~/") {
        if let Some(home) = home {
            return home.join(remainder);
        }
    }
    std::path::PathBuf::from(input)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LocalSendStatus {
    Pending,
    Failed,
}

#[derive(Clone, Debug)]
pub(super) struct LocalSubmission {
    pub command_id: u64,
    pub room: RoomId,
    pub draft: Draft,
    pub queued: bool,
    pub requested_at: std::time::Instant,
    pub submitted_at_ms: u64,
    pub status: LocalSendStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ComposerSize {
    #[default]
    Auto,
    Full,
    Compact,
}

/// A prompt this client saved for Up/Down recall, stamped so it orders
/// against the persisted prompts that also feed recall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Recalled {
    pub text: String,
    pub at_ms: u64,
}

impl Recalled {
    pub fn now(text: String) -> Self {
        Self {
            text,
            at_ms: crate::utils::time::now_ms(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct LocalRoom {
    pub text: Editor,
    pub recipients: AgentRecipients,
    pub composer_size: ComposerSize,
    // None follows the caret; Some is an independently scrolled viewport.
    pub composer_scroll: Option<usize>,
    pub recall: Vec<Recalled>,
    pub stash: Vec<String>,
    pub history_index: Option<usize>,
    pub live_draft: Option<String>,
    pub text_generation: u64,
    pub recipient_generation: u64,
    pub saved_text_generation: u64,
    pub saved_recipient_generation: u64,
}
impl From<&Room> for LocalRoom {
    fn from(room: &Room) -> Self {
        Self {
            text: Editor::new(room.draft.text.clone()),
            recipients: room.draft.recipient_ids.clone(),
            composer_size: ComposerSize::Auto,
            composer_scroll: None,
            recall: Vec::new(),
            stash: Vec::new(),
            history_index: None,
            live_draft: None,
            text_generation: 0,
            recipient_generation: 0,
            saved_text_generation: 0,
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
        self.request_send_with(room, true);
    }

    pub fn request_send(&mut self, room: RoomId) {
        self.request_send_with(room, false);
    }

    fn request_send_with(&mut self, room: RoomId, queued: bool) {
        if let Some(local) = self
            .locals
            .get_mut(&room)
            .filter(|local| local.text.text.trim() == "/help")
        {
            local.text = Editor::default();
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
            self.recipient_menu = true;
            self.recipient_index = 0;
            tracing::info!(
                event = "bus.message.recipients_requested",
                room_id = room.0,
                reason = "no_recipients",
                "Recipient picker opened before sending"
            );
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
                Effect::Files(id) if id != room => self.failed.push(pending),
                _ => {
                    self.queue(pending.command, pending.effect);
                }
            }
        }
        self.error = None;
        self.submissions.retain(|submission| {
            submission.room != room || submission.status != LocalSendStatus::Failed
        });
        let Some(draft) = self.draft_for_send(room) else {
            return;
        };
        let requested_at = std::time::Instant::now();
        let submitted_at_ms = crate::utils::time::now_ms();
        if let Some(local) = self.locals.get_mut(&room) {
            let text = local.text.text.clone();
            if !text.is_empty() && local.recall.last().map(|r| &r.text) != Some(&text) {
                local.recall.push(Recalled::now(text));
            }
            local.history_index = None;
            local.live_draft = None;
            local.text = Editor::default();
            local.text_generation += 1;
            local.composer_size = ComposerSize::Auto;
            local.composer_scroll = None;
        }
        let command = if queued {
            BusCommand::SubmitQueued(room, draft.clone())
        } else {
            BusCommand::Submit(room, draft.clone())
        };
        let command_id = self.queue(command, Effect::Submit);
        self.submissions.push(LocalSubmission {
            command_id,
            room,
            draft,
            queued,
            requested_at,
            submitted_at_ms,
            status: LocalSendStatus::Pending,
        });
        self.history_follow_tail = true;
    }

    fn draft_for_send(&self, room: RoomId) -> Option<Draft> {
        let local = self.locals.get(&room)?;
        Some(Draft {
            text: local.text.text.clone(),
            files: self.current_draft_files(room),
            recipient_ids: local.recipients.clone(),
        })
    }

    pub(super) fn current_draft_files(&self, room: RoomId) -> Vec<std::path::PathBuf> {
        let sending = self
            .submissions
            .iter()
            .filter(|submission| {
                submission.room == room && submission.status == LocalSendStatus::Pending
            })
            .flat_map(|submission| submission.draft.files.iter())
            .collect::<std::collections::BTreeSet<_>>();
        let consumed = |id: u64, path: &std::path::Path| {
            self.submissions.iter().any(|submission| {
                submission.room == room
                    && submission.status == LocalSendStatus::Pending
                    && submission.command_id > id
                    && submission.draft.files.iter().any(|sent| sent == path)
            })
        };
        let mut files: Vec<std::path::PathBuf> = self
            .snapshot
            .state
            .room(room)
            .map(|room| {
                room.draft
                    .files
                    .iter()
                    .filter(|path| !sending.contains(path))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for pending in &self.pending {
            match &pending.command {
                BusCommand::AttachFile(id, raw) if *id == room => {
                    let path = attachment_path(raw);
                    if !consumed(pending.id, &path) && !files.contains(&path) {
                        files.push(path);
                    }
                }
                BusCommand::RemoveFile(id, raw) if *id == room => {
                    let path = attachment_path(&raw.display().to_string());
                    files.retain(|file| file != &path);
                }
                _ => {}
            }
        }
        for pending in &self.failed {
            if let BusCommand::AttachFile(id, raw) = &pending.command {
                if *id == room {
                    let path = attachment_path(raw);
                    if !consumed(pending.id, &path) && !files.contains(&path) {
                        files.push(path);
                    }
                }
            }
        }
        files
    }

    /// The local text, or a file the coordinator holds or is still attaching.
    fn draft_has_content(&self, room: RoomId) -> bool {
        self.draft_for_send(room)
            .is_some_and(|draft| !draft.text.is_empty() || !draft.files.is_empty())
    }

    pub(super) fn restore_failed_submission(&mut self, command_id: u64) {
        let Some(submission) = self
            .submissions
            .iter_mut()
            .find(|submission| submission.command_id == command_id)
        else {
            return;
        };
        submission.status = LocalSendStatus::Failed;
        let room = submission.room;
        let sent = submission.draft.text.clone();
        if let Some(local) = self.locals.get_mut(&room) {
            let text = if sent.is_empty() {
                local.text.text.clone()
            } else if local.text.text.is_empty() {
                sent
            } else {
                format!("{sent}\n{}", local.text.text)
            };
            local.text = Editor::new(text);
            local.composer_scroll = None;
        }
        self.text_changed(room);
    }

    pub(super) fn history_projection(
        &self,
    ) -> (
        std::sync::Arc<crate::messaging::coordinator::BusSnapshot>,
        std::collections::BTreeMap<crate::messaging::model::PromptId, LocalSendStatus>,
    ) {
        let mut statuses = std::collections::BTreeMap::new();
        if self.submissions.is_empty() {
            return (std::sync::Arc::clone(&self.snapshot), statuses);
        }
        let mut projected = (*self.snapshot).clone();
        let mut local_revision = 1_u64 << 63;
        for submission in &self.submissions {
            local_revision = local_revision
                .wrapping_mul(31)
                .wrapping_add(submission.command_id)
                .wrapping_add(u64::from(submission.status == LocalSendStatus::Failed) << 32);
            let Ok(requests) = projected.state.submit_draft_exact(
                submission.room,
                submission.draft.clone(),
                submission.submitted_at_ms,
                submission.queued,
            ) else {
                continue;
            };
            if let Some(prompt) = requests
                .first()
                .and_then(|id| projected.state.request(*id))
                .map(|request| request.prompt.id)
            {
                statuses.insert(prompt, submission.status);
            }
        }
        projected.revision ^= local_revision;
        (std::sync::Arc::new(projected), statuses)
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

    /// Ctrl+C clears a visible room draft; it never quits Bus or cancels sends.
    pub(super) fn clear_composer(&mut self) -> bool {
        if self.quitting.is_some()
            || self.force_exit_until.is_some()
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
        if let Some(local) = self.locals.get_mut(&room) {
            local.text = Editor::default();
        }
        self.drag = None;
        self.text_changed(room);
        true
    }
}
