//! Draft/notes editing, key handling, recipients and image attachment persistence.
use super::super::{
    editor::Editor, forms::RenameTarget, render::Action, BusUi, ComposerSize, Effect,
};
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentId, AgentRecipients, RoomId, RoomKind},
};
use crossterm::event::{KeyCode, KeyModifiers};

impl BusUi {
    pub(super) fn scroll_composer(&mut self, forward: bool, lines: usize) {
        if let Some(local) = self.room.and_then(|room| self.locals.get_mut(&room)) {
            let offset = local.composer_scroll.unwrap_or(self.view.composer_scroll);
            let maximum = self
                .view
                .composer_rows
                .saturating_sub(usize::from(self.view.composer.height));
            local.composer_scroll = Some(if forward {
                offset.saturating_add(lines).min(maximum)
            } else {
                offset.min(maximum).saturating_sub(lines)
            });
        }
    }

    /// MASTER has no notes: they are a work room's status board, and MASTER is
    /// where the developer talks to orchestrators.
    pub(in crate::client::rooms) fn room_has_notes(&self) -> bool {
        self.room
            .and_then(|id| self.snapshot.state.room(id))
            .is_some_and(|room| room.kind != RoomKind::Master)
    }

    /// Starting to edit the notes brings their caret back into view.
    pub(super) fn reveal_notes_caret(&mut self) {
        if let Some(local) = self.room.and_then(|room| self.locals.get_mut(&room)) {
            local.notes_scroll = None;
        }
    }

    pub(super) fn scroll_notes(&mut self, forward: bool, lines: usize) {
        if let Some(local) = self.room.and_then(|room| self.locals.get_mut(&room)) {
            let offset = local.notes_scroll.unwrap_or(self.view.notes_scroll);
            let maximum = self
                .view
                .notes_rows
                .saturating_sub(usize::from(self.view.notes.height));
            local.notes_scroll = Some(if forward {
                offset.saturating_add(lines).min(maximum)
            } else {
                offset.min(maximum).saturating_sub(lines)
            });
        }
    }

    pub(super) fn toggle_recipient(&mut self, id: Option<AgentId>) {
        let Some(room) = self.room else {
            return;
        };
        let all: AgentRecipients = self
            .snapshot
            .state
            .agents()
            .filter(|a| a.room_id == room)
            .map(|a| a.id)
            .collect();
        if let Some(local) = self.locals.get_mut(&room) {
            if let Some(id) = id {
                if !local.recipients.remove(&id) {
                    local.recipients.insert(id);
                }
            } else if local.recipients.len() == all.len()
                && all.iter().all(|id| local.recipients.contains(id))
            {
                local.recipients.clear();
            } else {
                local.recipients = all;
            }
        }
        self.recipients_changed(room);
    }

    pub(super) fn insert(&mut self, text: &str) {
        // Host paste can use CR or CRLF; draft editors use LF for line breaks.
        let normalized = text
            .contains('\r')
            .then(|| text.replace("\r\n", "\n").replace('\r', "\n"));
        let text = normalized.as_deref().unwrap_or(text);
        if let Some(rename) = &mut self.rename {
            rename.editor.insert(text);
            return;
        }
        if let Some(form) = &mut self.form {
            if let Some(editor) = form.editor_mut() {
                editor.insert(text);
            }
            self.refill_orchestrator_prompt();
            self.query_paths();
            return;
        }
        let Some(room) = self.room else {
            return;
        };
        if let Some(local) = self.locals.get_mut(&room) {
            if self.notes_focus {
                local.notes.insert(text);
            } else {
                local.history_index = None;
                local.live_draft = None;
                local.text.insert(text);
            }
        }
        self.pending_line_continue = false;
        self.last_esc = None;
        if self.notes_focus {
            self.notes_changed(room);
        } else {
            self.text_changed(room);
        }
    }

    pub(super) fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.modal_key(code, modifiers) {
            return;
        }
        if !matches!(code, KeyCode::Esc) {
            self.last_esc = None;
        }
        if !self.composer_command_key(code, modifiers) {
            self.edit_composer_key(code, modifiers);
        }
    }

    fn modal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if self.rename.is_some() {
            self.rename_key(code, modifiers);
            return true;
        }
        if self.form.is_some() {
            self.form_key(code, modifiers);
            return true;
        }
        if self.recipient_menu {
            self.recipient_menu_key(code);
            return true;
        }
        if self.chat_search.is_some() {
            self.chat_search_key(code, modifiers);
            return true;
        }
        if self.history_search.is_some() && !self.notes_focus {
            self.search_key(code, modifiers);
            return true;
        }
        false
    }

    fn rename_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Esc => self.rename = None,
            KeyCode::Enter => {
                if let Some(rename) = self.rename.take() {
                    let cmd = match rename.target {
                        RenameTarget::Room(id) => BusCommand::RenameRoom(id, rename.editor.text),
                        RenameTarget::Agent(id) => BusCommand::RenameAgent(id, rename.editor.text),
                    };
                    self.queue(cmd, Effect::None);
                }
            }
            KeyCode::Char(c)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.insert(&c.to_string())
            }
            _ => {
                if let Some(rename) = &mut self.rename {
                    rename.editor.key(code, modifiers);
                }
            }
        }
    }

    fn recipient_menu_key(&mut self, code: KeyCode) {
        let ids: Vec<_> = self
            .snapshot
            .state
            .agents()
            .filter(|a| Some(a.room_id) == self.room)
            .map(|a| a.id)
            .collect();
        match code {
            KeyCode::Esc | KeyCode::Tab => self.recipient_menu = false,
            KeyCode::Up => self.recipient_index = self.recipient_index.saturating_sub(1),
            KeyCode::Down => self.recipient_index = (self.recipient_index + 1).min(ids.len()),
            KeyCode::Enter | KeyCode::Char(' ') => match self.recipient_index.checked_sub(1) {
                None => self.toggle_recipient(None),
                Some(i) => {
                    // A snapshot can delete the highlighted agent; its stale
                    // row selects nothing rather than falling back to All.
                    if let Some(&id) = ids.get(i) {
                        self.toggle_recipient(Some(id));
                    }
                }
            },
            _ => {}
        }
    }

    fn composer_command_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match (code, modifiers) {
            (KeyCode::F(2), _) => {
                if let Some(id) = self.terminal {
                    self.start_rename(RenameTarget::Agent(id));
                } else if let Some(id) = self.room {
                    self.start_rename(RenameTarget::Room(id));
                }
            }
            (KeyCode::F(3), _) => {
                if self.room_has_notes() {
                    if !self.notes_focus {
                        self.reveal_notes_caret();
                    }
                    self.notes_focus = !self.notes_focus;
                }
            }
            (KeyCode::Char('e' | 'E'), modifiers)
                if modifiers.contains(KeyModifiers::CONTROL)
                    && modifiers.contains(KeyModifiers::SHIFT)
                    && !self.notes_focus =>
            {
                if let Some(local) = self.room.and_then(|room| self.locals.get_mut(&room)) {
                    local.composer_size = if local.composer_size == ComposerSize::Full
                        || (self.view.composer.height > 0
                            && self.view.composer.height == self.view.composer_max_height)
                    {
                        ComposerSize::Compact
                    } else {
                        ComposerSize::Full
                    };
                }
            }
            (KeyCode::PageUp | KeyCode::PageDown, KeyModifiers::NONE) if self.notes_focus => {
                self.scroll_notes(
                    code == KeyCode::PageDown,
                    usize::from(self.view.notes.height.saturating_sub(1).max(1)),
                );
            }
            (KeyCode::PageUp | KeyCode::PageDown, KeyModifiers::NONE) if !self.notes_focus => {
                self.scroll_composer(
                    code == KeyCode::PageDown,
                    usize::from(self.view.composer.height.saturating_sub(1).max(1)),
                );
            }
            (KeyCode::Char('r' | 'R'), KeyModifiers::CONTROL) if !self.notes_focus => {
                self.cycle_history_search();
            }
            (KeyCode::Char('r' | 'R'), modifiers)
                if modifiers.contains(KeyModifiers::CONTROL)
                    && modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.action(Action::NewRoom);
            }
            (KeyCode::Char('s' | 'S'), KeyModifiers::CONTROL) if !self.notes_focus => {
                self.stash_prompt();
            }
            (KeyCode::Char('v' | 'V'), KeyModifiers::CONTROL) if !self.notes_focus => {
                self.paste_image();
            }
            (KeyCode::Char('n'), KeyModifiers::CONTROL) => self.action(Action::NewAgent),
            // Ctrl+F finds in the chat history, as in most apps; Ctrl+O, free in
            // Bus and in iTerm2, Ghostty and Windows Terminal, adds files.
            (KeyCode::Char('f' | 'F'), KeyModifiers::CONTROL) => self.open_chat_search(),
            (KeyCode::Char('o' | 'O'), KeyModifiers::CONTROL) => self.action(Action::Files),
            // Pickers sit on Ctrl chords: every printable key, shifted symbols
            // like @ and + included, must type into the composer.
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => self.action(Action::Recipients),
            _ => return false,
        }
        true
    }

    fn edit_composer_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match (code, modifiers) {
            (KeyCode::Char('j'), KeyModifiers::CONTROL) | (KeyCode::Enter, KeyModifiers::SHIFT) => {
                self.insert("\n")
            }
            (KeyCode::Char('\\'), modifiers)
                if !self.notes_focus && modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                self.insert("\\");
                self.pending_line_continue = true;
            }
            (KeyCode::Enter, _) if self.notes_focus => self.insert("\n"),
            (KeyCode::Enter, _) if self.pending_line_continue => self.finish_line_continue(),
            (KeyCode::Enter, KeyModifiers::ALT) => {
                if let Some(room) = self.room {
                    self.request_queued_send(room);
                }
            }
            (KeyCode::Enter, _) => {
                if let Some(room) = self.room {
                    self.request_send(room);
                }
            }
            (KeyCode::Up | KeyCode::Down, KeyModifiers::NONE) if !self.notes_focus => {
                self.history_or_move(code);
            }
            (KeyCode::Esc, _) => self.escape(),
            (KeyCode::Char(c), _)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.insert(&c.to_string())
            }
            _ => {
                if let Some(room) = self.room {
                    if let Some(local) = self.locals.get_mut(&room) {
                        if self.notes_focus {
                            local.notes_scroll = None;
                        } else {
                            local.composer_scroll = None;
                        }
                        let editor = if self.notes_focus {
                            &mut local.notes
                        } else {
                            &mut local.text
                        };
                        let changed = editor.key(code, modifiers);
                        if changed {
                            // Same invalidation as `insert`: edited text is no
                            // longer the recalled prompt or a trailing `\`.
                            if !self.notes_focus {
                                local.history_index = None;
                                local.live_draft = None;
                            }
                            self.pending_line_continue = false;
                            if self.notes_focus {
                                self.notes_changed(room);
                            } else {
                                self.text_changed(room);
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn escape(&mut self) {
        if self.notes_focus {
            self.notes_focus = false;
            self.last_esc = None;
            return;
        }
        let now = std::time::Instant::now();
        if self.last_esc.is_some_and(|last| {
            now.saturating_duration_since(last) < std::time::Duration::from_millis(800)
        }) {
            self.archive_draft();
            self.last_esc = None;
        } else {
            self.last_esc = Some(now);
        }
    }

    pub(super) fn archive_draft(&mut self) {
        let Some(room) = self.room else {
            return;
        };
        let Some(local) = self.locals.get_mut(&room) else {
            return;
        };
        let text = std::mem::take(&mut local.text.text);
        if !text.is_empty() && local.recall.last() != Some(&text) {
            local.recall.push(text);
        }
        local.text = Editor::default();
        local.history_index = None;
        local.live_draft = None;
        self.text_changed(room);
    }

    pub(super) fn stash_prompt(&mut self) {
        let Some(room) = self.room else {
            return;
        };
        let Some(local) = self.locals.get_mut(&room) else {
            return;
        };
        if local.text.text.is_empty() {
            if let Some(text) = local.stash.pop() {
                local.text = Editor::new(text);
                local.history_index = None;
                local.live_draft = None;
                self.text_changed(room);
            }
            return;
        }
        local.stash.push(std::mem::take(&mut local.text.text));
        local.text = Editor::default();
        local.history_index = None;
        local.live_draft = None;
        self.text_changed(room);
    }

    pub(super) fn finish_line_continue(&mut self) {
        self.pending_line_continue = false;
        let Some(room) = self.room else {
            return;
        };
        if let Some(local) = self.locals.get_mut(&room) {
            if local.text.cursor > 0 && local.text.text[..local.text.cursor].ends_with('\\') {
                local.text.key(KeyCode::Backspace, KeyModifiers::NONE);
            }
            local.text.insert("\n");
        }
        self.text_changed(room);
    }

    pub(super) fn paste_image(&mut self) {
        let Some(image) = crate::platform::read_clipboard_image(
            crate::protocol::wire::MAX_CLIPBOARD_IMAGE_PAYLOAD,
        ) else {
            return;
        };
        self.attach_image_data(
            crate::utils::env::bus_data_dir(),
            &image.bytes,
            image.extension,
        );
    }

    /// Saves pasted image data under `root` (the Bus data directory) and
    /// attaches the saved file to the open room's draft.
    pub(in crate::client::rooms) fn attach_image_data(
        &mut self,
        root: Option<std::path::PathBuf>,
        bytes: &[u8],
        extension: &str,
    ) {
        let Some(room) = self.room else {
            return;
        };
        if let Some(error) = super::super::thumbnails::image_data_error(bytes) {
            tracing::warn!(event = "bus.attach.image_refused", %error);
            self.show_alert(
                "Could not attach the clipboard image".into(),
                format!("The clipboard image is not a readable image, so it was not attached.\n\n{error}"),
            );
            return;
        }
        let Some(root) = root else {
            self.error = Some("Could not save the clipboard image: no Bus data directory.".into());
            return;
        };
        let path = match save_pasted_image(&root, room, bytes, extension) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(event = "bus.paste_image.write_failed", %error);
                self.error = Some("Could not write the clipboard image.".into());
                return;
            }
        };
        self.queue(
            BusCommand::AttachFile(room, path.display().to_string()),
            Effect::Files(room),
        );
    }

    /// Attaches dropped, pasted or typed paths to the room's draft, except
    /// images Bus cannot decode: history would show those as blank rows, so a
    /// dialog names each refused file and its decode error instead.
    pub(in crate::client::rooms) fn attach_paths(&mut self, room: RoomId, paths: Vec<String>) {
        let mut refused = Vec::new();
        for path in paths {
            let file = crate::utils::home_path::expand_tilde_path(&path);
            if let Some(error) = super::super::thumbnails::image_file_error(&file) {
                tracing::warn!(event = "bus.attach.image_refused", %error);
                refused.push((file, error));
                continue;
            }
            let path = self.keep_temporary_image(room, path);
            self.queue(BusCommand::AttachFile(room, path), Effect::Files(room));
        }
        let name = |file: &std::path::Path| {
            file.file_name().map_or_else(
                || file.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        };
        match refused.as_slice() {
            [] => {}
            [(file, error)] => self.show_alert(
                format!("Could not attach \"{}\"", name(file)),
                format!(
                    "{} is not a readable image, so it was not attached.\n\n{error}",
                    file.display()
                ),
            ),
            refused => self.show_alert(
                format!("Could not attach {} images", refused.len()),
                std::iter::once(
                    "These are not readable images, so they were not attached:".to_owned(),
                )
                .chain(
                    refused
                        .iter()
                        .map(|(file, error)| format!("\n{}: {error}", file.display())),
                )
                .collect(),
            ),
        }
    }

    /// A pasted image path in the OS temp folder (a macOS screenshot or a
    /// terminal's image paste) vanishes once its app cleans up, and the message
    /// would then lose the image. Bus keeps its own copy under the room's
    /// attachments, as for a clipboard image; any other path is kept as given.
    pub(super) fn keep_temporary_image(&mut self, room: RoomId, path: String) -> String {
        let Some(root) = crate::utils::env::bus_data_dir() else {
            return path;
        };
        match copy_temporary_image(&root, room, std::path::Path::new(&path)) {
            Ok(Some(copy)) => copy.display().to_string(),
            Ok(None) => path,
            Err(error) => {
                tracing::warn!(event = "bus.paste_image.copy_failed", %error);
                path
            }
        }
    }
}

/// Saves a pasted clipboard image where later readers can still open it: the
/// Bus data directory outlives the session, unlike the system temp directory.
/// Naming by content keeps repeated pastes of one image to a single file.
/// Copies `path` into the room's attachments when it is an image in a
/// temporary folder; `Ok(None)` leaves any other path as it is.
pub(in crate::client::rooms) fn copy_temporary_image(
    root: &std::path::Path,
    room: RoomId,
    path: &std::path::Path,
) -> std::io::Result<Option<std::path::PathBuf>> {
    if !super::super::thumbnails::is_image(path) {
        return Ok(None);
    }
    let Ok(canonical) = path.canonicalize() else {
        return Ok(None);
    };
    // Bus's own data dir can itself live in a temp folder (tests, isolated runs).
    let owned = root
        .canonicalize()
        .is_ok_and(|root| canonical.starts_with(root));
    let temporary = [std::env::temp_dir(), "/tmp".into()]
        .into_iter()
        .filter_map(|dir| dir.canonicalize().ok())
        .any(|dir| canonical.starts_with(dir));
    if owned || !temporary {
        return Ok(None);
    }
    let extension = canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    save_pasted_image(root, room, &std::fs::read(&canonical)?, &extension).map(Some)
}

pub(in crate::client::rooms) fn save_pasted_image(
    root: &std::path::Path,
    room: RoomId,
    bytes: &[u8],
    extension: &str,
) -> std::io::Result<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let dir = root.join("attachments").join(format!("room-{}", room.0));
    std::fs::create_dir_all(&dir)?;
    let digest = format!("{:x}", Sha256::digest(bytes));
    let path = dir.join(format!("paste-{}.{extension}", &digest[..16]));
    if !path.is_file() {
        // Write beside the target and rename so readers never see a partial image.
        let partial = dir.join(format!(".{}.partial-{}", &digest[..16], std::process::id()));
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(path)
}
