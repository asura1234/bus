//! Room input and action dispatch.
mod composer;
mod forms;
mod history;
mod settings;

#[cfg(test)]
pub(super) use composer::{copy_temporary_image, save_pasted_image};

use super::{
    editor::Editor,
    forms::{Form, Orchestrates, PromptField, RenameTarget, ORCHESTRATES_FIELD},
    render::Action,
};
use super::{BusUi, Effect};
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentId, RoomId, RoomKind},
};
use crate::{
    client::compositor::ClientShellInput,
    protocol::keys::{host::RawInputEvent, TerminalKey},
};
use crossterm::event::{
    KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

impl BusUi {
    pub fn input(
        &mut self,
        event: &RawInputEvent,
        ready: bool,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if let RawInputEvent::Key(key) = event {
            if self.global_room_key(key, event, outcome) {
                return true;
            }
        }
        if self.deletion_input(event) {
            outcome.repaint = true;
            return true;
        }
        if self.quitting.is_some()
            && matches!(
                event,
                RawInputEvent::Key(_)
                    | RawInputEvent::Text(_)
                    | RawInputEvent::Paste(_)
                    | RawInputEvent::Mouse(_)
            )
        {
            return true;
        }

        if let RawInputEvent::Mouse(mouse) = event {
            if let Some(consumed) = self.room_mouse(mouse, ready, outcome) {
                return consumed;
            }
        }
        if self.terminal.is_some() && self.form.is_none() && self.rename.is_none() {
            return matches!(
                event,
                RawInputEvent::Key(_)
                    | RawInputEvent::Text(_)
                    | RawInputEvent::Paste(_)
                    | RawInputEvent::Mouse(_)
            ) && !ready;
        }
        self.edit_room_event(event, outcome)
    }

    fn global_room_key(
        &mut self,
        key: &TerminalKey,
        event: &RawInputEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        // Ctrl+C never quits Bus. A focused agent terminal receives it
        // unchanged below so the human can interrupt the agent; the room
        // composer clears its draft; everywhere else it does nothing.
        let interrupt =
            matches!(key.code, KeyCode::Char('c' | 'C')) && key.modifiers == KeyModifiers::CONTROL;
        if interrupt && !self.terminal_has_focus() {
            if key.kind != KeyEventKind::Release && self.clear_composer() {
                outcome.repaint = true;
            }
            return true;
        }
        let quit = matches!(key.code, KeyCode::Char('q' | 'Q'))
            && key.modifiers.contains(KeyModifiers::CONTROL);
        if quit && key.kind != KeyEventKind::Release {
            if key.modifiers.contains(KeyModifiers::SHIFT) && self.force_exit_available {
                outcome.detach = true;
            } else {
                self.request_quit();
                outcome.repaint = true;
            }
            return true;
        }
        if self.deletion.is_some() {
            self.deletion_input(event);
            outcome.repaint = true;
            return true;
        }
        if key.code == KeyCode::F(6) {
            if let Some(room) = self.room {
                self.open_room(room);
            }
            outcome.repaint = true;
            return true;
        }
        if key.code == KeyCode::F(2) && self.form.is_none() && self.rename.is_none() {
            if let Some(agent) = self.terminal {
                self.start_rename(RenameTarget::Agent(agent));
            } else if let Some(room) = self.room {
                self.start_rename(RenameTarget::Room(room));
            }
            outcome.repaint = true;
            return true;
        }
        false
    }

    fn room_mouse(
        &mut self,
        mouse: &MouseEvent,
        ready: bool,
        outcome: &mut ClientShellInput,
    ) -> Option<bool> {
        let hit = self
            .view
            .hits
            .iter()
            .rev()
            .find(|h| h.rect.contains((mouse.column, mouse.row).into()))
            .map(|h| h.action.clone());
        if mouse.kind == MouseEventKind::Moved {
            let detail = match &hit {
                Some(Action::RemoveFile(path) | Action::FileDetail(path)) => {
                    Some(path.display().to_string())
                }
                _ => None,
            };
            if self.detail_path != detail {
                self.detail_path = detail;
                outcome.repaint = true;
            }
        }
        if self.selection_mouse(mouse, hit.as_ref(), outcome) {
            outcome.repaint = true;
            return Some(true);
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(action) = hit {
                let double = self.last_click.as_ref().is_some_and(|(last, time)| {
                    *last == action && time.elapsed() < std::time::Duration::from_millis(400)
                });
                self.last_click = Some((action.clone(), std::time::Instant::now()));
                if double {
                    match action {
                        Action::Room(room) => self.start_rename(RenameTarget::Room(room)),
                        Action::Agent(agent) => self.start_rename(RenameTarget::Agent(agent)),
                        _ => self.action(action),
                    }
                } else {
                    self.action(action);
                }
                outcome.repaint = true;
                return Some(true);
            }
        }
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
        ) {
            return Some(self.scroll_room_mouse(mouse, ready, outcome));
        }
        if mouse.column < self.view.sidebar.right() {
            return Some(true);
        }
        None
    }

    fn scroll_overlay_mouse(&mut self, mouse: &MouseEvent, outcome: &mut ClientShellInput) -> bool {
        if matches!(self.form, Some(Form::Help { .. })) && mouse.column >= self.view.sidebar.right()
        {
            self.scroll_help(mouse.kind == MouseEventKind::ScrollDown, 3);
            outcome.repaint = true;
            return true;
        }
        if matches!(self.form, Some(Form::Settings)) && mouse.column >= self.view.sidebar.right() {
            self.settings_scroll = if mouse.kind == MouseEventKind::ScrollDown {
                (self.settings_scroll + 3).min(self.view.settings_max_scroll)
            } else {
                self.settings_scroll.saturating_sub(3)
            };
            outcome.repaint = true;
            return true;
        }
        if self.terminal.is_none()
            && self.form.is_none()
            && self
                .view
                .recipient_bar
                .contains((mouse.column, mouse.row).into())
        {
            self.recipient_scroll = if mouse.kind == MouseEventKind::ScrollDown {
                self.recipient_scroll
                    .saturating_add(3)
                    .min(self.view.recipient_max_scroll)
            } else {
                self.recipient_scroll.saturating_sub(3)
            };
            outcome.repaint = true;
            return true;
        }
        if self.terminal.is_none()
            && self.form.is_none()
            && self
                .view
                .notes_box
                .contains((mouse.column, mouse.row).into())
        {
            self.scroll_notes(mouse.kind == MouseEventKind::ScrollDown, 1);
            outcome.repaint = true;
            return true;
        }
        if self.terminal.is_none()
            && !self.recipient_menu
            && self
                .view
                .composer
                .contains((mouse.column, mouse.row).into())
        {
            self.scroll_composer(mouse.kind == MouseEventKind::ScrollDown, 3);
            outcome.repaint = true;
            return true;
        }
        false
    }

    fn scroll_room_mouse(
        &mut self,
        mouse: &MouseEvent,
        ready: bool,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.scroll_overlay_mouse(mouse, outcome) {
            return true;
        }
        let file_row = self.view.files.contains((mouse.column, mouse.row).into());
        let history_row = self.view.history.contains((mouse.column, mouse.row).into());
        if file_row {
            self.detail_path = None;
        }
        let (scroll, maximum) = if mouse.column < self.view.sidebar.right() {
            (&mut self.sidebar_scroll, self.view.sidebar_max_scroll)
        } else if file_row {
            (&mut self.file_scroll, usize::MAX)
        } else if history_row {
            (&mut self.main_scroll, self.view.history_max_scroll)
        } else if self.terminal.is_none() || self.form.is_some() {
            return true;
        } else {
            return !ready;
        };
        let step = if file_row || history_row { 1 } else { 3 };
        *scroll = if mouse.kind == MouseEventKind::ScrollDown {
            scroll.saturating_add(step).min(maximum)
        } else {
            scroll.saturating_sub(step)
        };
        if history_row {
            self.history_follow_tail = self.main_scroll == self.view.history_max_scroll;
        }
        outcome.repaint = true;
        true
    }

    fn edit_room_event(&mut self, event: &RawInputEvent, outcome: &mut ClientShellInput) -> bool {
        match event {
            RawInputEvent::Text(text) => {
                self.insert(text.as_str());
                self.keep_focused_selection(None);
            }
            RawInputEvent::Paste(text) => {
                self.paste_room_text(text);
                self.keep_focused_selection(None);
            }
            RawInputEvent::Key(key) => {
                if self.room_editor_key(key, outcome) {
                    return true;
                }
            }
            RawInputEvent::Mouse(_) => {}
            _ => return false,
        }
        outcome.repaint = true;
        true
    }

    fn paste_room_text(&mut self, text: &str) {
        if self.form.is_none() && self.rename.is_none() && !self.notes_focus {
            let paths = crate::messaging::attachments::parse_path_tokens(text)
                .ok()
                .filter(|paths| {
                    text.trim() != "/help"
                        && !paths.is_empty()
                        && paths
                            .iter()
                            .all(|p| p.starts_with('/') || p.starts_with("~/"))
                });
            if let (Some(room), Some(paths)) = (self.room, paths) {
                for path in paths {
                    let path = self.keep_temporary_image(room, path);
                    self.queue(BusCommand::AttachFile(room, path), Effect::Files(room));
                }
            } else {
                self.insert(text);
            }
        } else {
            self.insert(text);
        }
    }

    fn room_editor_key(&mut self, key: &TerminalKey, outcome: &mut ClientShellInput) -> bool {
        if key.kind == KeyEventKind::Release {
            return true;
        }
        // Terminals that forward Cmd+C can copy the selection again.
        if matches!(key.code, KeyCode::Char('c' | 'C')) && key.modifiers == KeyModifiers::SUPER {
            if let Some(text) = self.selected_text() {
                outcome.actions.push(
                    crate::client::compositor::ClientShellAction::ClipboardWrite(text.into_bytes()),
                );
            }
            return true;
        }
        if matches!(key.code, KeyCode::Char('g' | 'G'))
            && key.modifiers == KeyModifiers::CONTROL
            && self.form.is_none()
            && self.rename.is_none()
            && self.terminal.is_none()
            && !self.notes_focus
        {
            outcome
                .actions
                .push(crate::client::compositor::ClientShellAction::EditComposer);
            outcome.repaint = true;
            return true;
        }
        // Enhanced terminals can report the base key plus its shifted
        // symbol separately (e.g. 2 + Shift with an alternate @).
        let code = if key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char(_))
        {
            key.shifted_codepoint
                .and_then(char::from_u32)
                .map(KeyCode::Char)
                .unwrap_or(key.code)
        } else {
            key.code
        };
        self.key(code, key.modifiers);
        self.keep_focused_selection(Some(key.code));
        false
    }

    /// An agent terminal is open with no Bus overlay taking its keys.
    fn terminal_has_focus(&self) -> bool {
        self.terminal.is_some()
            && self.deletion.is_none()
            && self.form.is_none()
            && self.rename.is_none()
    }
    pub fn terminal_ready(&self, pane: Option<&str>) -> bool {
        self.terminal.is_some()
            && self.deletion.is_none()
            && self.form.is_none()
            && self
                .target_pane
                .as_deref()
                .is_some_and(|target| Some(target) == pane)
    }

    pub fn open_terminal(&mut self, agent: AgentId) {
        self.clear_selection();
        self.terminal = Some(agent);
        self.target_pane = None;
        self.form = None;
        self.recipient_menu = false;
        self.queue(BusCommand::LeaveRoom, Effect::None);
        self.queue(BusCommand::FocusTerminal(agent), Effect::None);
    }
    pub fn open_room(&mut self, room: RoomId) {
        self.clear_selection();
        if let Some(row) = super::render::sidebar_room_row(&self.snapshot.state, room, Some(room))
            .filter(|_| self.view.sidebar_body.height > 0)
        {
            let capacity = usize::from(self.view.sidebar_body.height.max(1));
            if row < self.sidebar_scroll {
                self.sidebar_scroll = row;
            } else if row >= self.sidebar_scroll + capacity {
                self.sidebar_scroll = row + 1 - capacity;
            }
        }
        self.file_scroll = 0;
        self.detail_path = None;
        self.room = Some(room);
        self.terminal = None;
        self.target_pane = None;
        self.form = None;
        self.rename = None;
        self.notes_focus = false;
        self.recipient_menu = false;
        self.main_scroll = 0;
        self.history_follow_tail = true;
        self.recipient_scroll = 0;
        self.queue(BusCommand::SelectRoom(room), Effect::None);
    }
    pub(super) fn is_master_room(&self, room: RoomId) -> bool {
        self.snapshot
            .state
            .room(room)
            .is_some_and(|room| room.kind == RoomKind::Master)
    }
    fn open_agent_form(&mut self) {
        let master = self.room.is_some_and(|room| self.is_master_room(room));
        self.open_form(Form::Agent {
            name: Editor::default(),
            provider: None,
            provider_cursor: super::forms::provider_choices(master)[0],
            cwd: Editor::new("~/".into()),
            args: Box::new(Editor::default()),
            field: 0,
            // Starts on the first work room without an orchestrator.
            orchestrates: master
                .then(|| Orchestrates(self.orchestratable_rooms().first().copied())),
            prompt: master.then(|| {
                Box::new(PromptField {
                    editor: Editor::default(),
                    filled: String::new(),
                })
            }),
        });
        self.refill_orchestrator_prompt();
    }

    fn open_sound_settings(&mut self) {
        self.settings_field = 0;
        self.settings_scroll = 0;
        if self.system_sounds.is_none() {
            self.system_sounds = Some(
                crate::platform::sound::system_sounds()
                    .into_iter()
                    .map(|sound| sound.name)
                    .collect(),
            );
        }
        self.open_form(Form::Settings);
    }

    fn remove_room_file(&mut self, path: std::path::PathBuf) {
        if let Some(room) = self.room {
            let before = self.failed.len();
            self.failed.retain(|p| !matches!(&p.command,BusCommand::AttachFile(id,input) if *id==room && std::path::Path::new(input)==path));
            if before != self.failed.len() {
                self.error = None;
                return;
            }
            self.queue(BusCommand::RemoveFile(room, path), Effect::Files(room));
        }
    }

    fn focus_sound_setting(&mut self, target: super::render::SoundTarget) {
        if let Some(index) = self
            .sound_settings_targets()
            .iter()
            .position(|t| *t == target)
        {
            self.settings_field = index + 1;
        }
    }

    pub(super) fn action(&mut self, action: Action) {
        match action {
            Action::Delete(target) => self.start_delete(target),
            Action::CancelDelete => self.cancel_delete(),
            Action::ConfirmDelete => self.confirm_delete(),
            Action::ScrollFiles(forward) => {
                self.file_scroll = if forward {
                    self.file_scroll.saturating_add(1)
                } else {
                    self.file_scroll.saturating_sub(1)
                };
                self.detail_path = None;
            }
            Action::Room(room) => self.open_room(room),
            Action::Agent(agent) => self.open_terminal(agent),
            Action::NewRoom => self.open_form(Form::Room(Editor::default())),
            Action::NewAgent => self.open_agent_form(),
            Action::Orchestrates => {
                if let Some(Form::Agent { field, .. }) = &mut self.form {
                    *field = ORCHESTRATES_FIELD;
                }
                self.cycle_orchestrates(true);
            }
            Action::Notes => {
                if self.room_has_notes() {
                    if !self.notes_focus {
                        self.reveal_notes_caret();
                    }
                    self.notes_focus = true;
                    self.recipient_menu = false;
                }
            }
            Action::Composer => {
                self.notes_focus = false;
                self.recipient_menu = false;
            }
            Action::Recipients => {
                self.recipient_menu = !self.recipient_menu;
                self.notes_focus = false;
            }
            Action::Recipient(id) => self.toggle_recipient(id),
            Action::Files => self.open_form(Form::Files(Editor::new("~/".into()))),
            Action::RemoveFile(path) => self.remove_room_file(path),
            Action::FileDetail(path) => self.detail_path = Some(path.display().to_string()),
            Action::Details(agent) => {
                if let Some(a) = self.snapshot.state.agent(agent) {
                    self.queue(
                        BusCommand::SetDetails(agent, !a.details_disclosed),
                        Effect::None,
                    );
                }
            }
            Action::Quote(agent) => self.quote(agent),
            Action::Field(index) => {
                if let Some(Form::Agent { field, .. }) = &mut self.form {
                    *field = index;
                }
                self.query_paths();
            }
            Action::Provider(kind) => {
                if let Some(Form::Agent {
                    provider,
                    provider_cursor,
                    field,
                    ..
                }) = &mut self.form
                {
                    *provider = Some(kind);
                    *provider_cursor = kind;
                    *field = 2;
                }
                self.query_paths();
            }
            Action::Suggestion(index) => {
                self.suggestions.selected = index;
                self.complete_path();
            }
            Action::Settings => self.open_sound_settings(),
            Action::ToggleSound(target) => {
                self.focus_sound_setting(target);
                self.toggle_sound(target);
            }
            Action::CycleSound(target, forward) => {
                self.focus_sound_setting(target);
                self.cycle_sound(target, forward);
            }
            Action::ToggleColorBlindMode => self.toggle_color_blind_mode(),
            Action::AdjustCompactionLimit(forward) => self.adjust_compaction_limit(forward),
            Action::Cancel => {
                if let Some(room) = self.room {
                    self.open_room(room);
                } else {
                    self.form = None;
                }
            }
            Action::Add => self.add(),
        }
    }
}
