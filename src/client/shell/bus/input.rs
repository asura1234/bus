use super::*;
use super::{
    editor::Editor,
    forms::{Form, Rename, RenameTarget},
    render::Action,
};
use crate::bus::{launch::AddAgent, model::*, runtime::BusCommand};
use crate::{client::shell::ClientShellInput, raw_input::RawInputEvent};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use std::collections::BTreeSet;

impl BusUi {
    pub fn input(
        &mut self,
        event: &RawInputEvent,
        ready: bool,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if let RawInputEvent::Key(key) = event {
            let quit = (matches!(key.code, KeyCode::Char('c' | 'C'))
                && key.modifiers == KeyModifiers::CONTROL)
                || (matches!(key.code, KeyCode::Char('q' | 'Q'))
                    && key.modifiers.contains(KeyModifiers::CONTROL));
            if quit && key.kind != KeyEventKind::Release {
                if matches!(key.code, KeyCode::Char('c' | 'C')) && self.clear_composer() {
                    outcome.repaint = true;
                    return true;
                }
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
                return true;
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
                    return true;
                }
            }
            if matches!(
                mouse.kind,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
            ) {
                if matches!(self.form, Some(Form::Help { .. }))
                    && mouse.column >= self.view.sidebar.right()
                {
                    self.scroll_help(mouse.kind == MouseEventKind::ScrollDown, 3);
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
                return true;
            }
            if mouse.column < self.view.sidebar.right() {
                return true;
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
        match event {
            RawInputEvent::Text(text) => {
                self.insert(text.as_str());
                self.keep_focused_selection(None);
            }
            RawInputEvent::Paste(text) => {
                if self.form.is_none() && self.rename.is_none() && !self.notes_focus {
                    let paths = crate::bus::files::parse_path_tokens(text)
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
                            self.queue(BusCommand::AttachFile(room, path), Effect::Files(room));
                        }
                    } else {
                        self.insert(text);
                    }
                } else {
                    self.insert(text);
                }
                self.keep_focused_selection(None);
            }
            RawInputEvent::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return true;
                }
                // Terminals that forward Cmd+C can copy the selection again.
                if matches!(key.code, KeyCode::Char('c' | 'C'))
                    && key.modifiers == KeyModifiers::SUPER
                {
                    if let Some(text) = self.selected_text() {
                        outcome.actions.push(
                            crate::client::shell::ClientShellAction::ClipboardWrite(
                                text.into_bytes(),
                            ),
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
                        .push(crate::client::shell::ClientShellAction::EditComposer);
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
            }
            RawInputEvent::Mouse(_) => {}
            _ => return false,
        }
        outcome.repaint = true;
        true
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
    fn scroll_composer(&mut self, forward: bool, lines: usize) {
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
    fn scroll_help(&mut self, forward: bool, lines: usize) {
        if let Some(Form::Help { scroll }) = &mut self.form {
            *scroll = if forward {
                self.view
                    .help_scroll
                    .saturating_add(lines)
                    .min(self.view.help_max_scroll)
            } else {
                self.view.help_scroll.saturating_sub(lines)
            };
        }
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
        if let Some(index) = self
            .snapshot
            .state
            .rooms()
            .position(|r| r.id == room)
            .filter(|_| self.view.sidebar_body.height > 0)
        {
            let row = index + 3;
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
    pub fn quote(&mut self, request: RequestId) {
        let Some(room) = self.room else {
            return;
        };
        let Some((agent, text)) = super::history::reply(&self.snapshot.state, room, request) else {
            return;
        };
        let Some(name) = self.snapshot.state.agent(agent).map(|a| a.name.as_str()) else {
            return;
        };
        let quote = format!(
            "{name}: \"{}\"\n",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        );
        if let Some(local) = self.locals.get_mut(&room) {
            // A leftover mouse selection must not be replaced by the quote.
            local.text.anchor = None;
            local.text.cursor = local.text.text.len();
            if !local.text.text.is_empty() && !local.text.text.ends_with('\n') {
                local.text.insert("\n");
            }
            local.text.insert(&quote);
        }
        self.notes_focus = false;
        self.recipient_menu = false;
        self.text_changed(room);
    }
    fn start_rename(&mut self, target: RenameTarget) {
        let name = match target {
            RenameTarget::Room(id) => self.snapshot.state.room(id).map(|r| r.name.clone()),
            RenameTarget::Agent(id) => self.snapshot.state.agent(id).map(|a| a.name.clone()),
        };
        if let Some(name) = name {
            self.rename = Some(Rename {
                target,
                editor: Editor::new(name),
            });
        }
    }
    fn open_form(&mut self, form: Form) {
        self.clear_selection();
        if self.failed.is_empty() {
            // Dismiss only a handled UI operation's matching snapshot copy,
            // not unrelated background/runtime failures or unsaved drafts.
            if self.error.is_some() && self.error == self.snapshot.error {
                self.dismissed_snapshot_error = self.error.clone();
            }
            self.error = None;
        }
        self.form = Some(form);
        self.terminal = None;
        self.recipient_menu = false;
        self.queue(BusCommand::LeaveRoom, Effect::None);
        self.query_paths();
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
            Action::NewAgent => self.open_form(Form::Agent {
                name: Editor::default(),
                provider: None,
                provider_cursor: Provider::Codex,
                cwd: Editor::new("~/".into()),
                args: Box::new(Editor::default()),
                field: 0,
            }),
            Action::Notes => {
                self.notes_focus = true;
                self.recipient_menu = false;
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
            Action::RemoveFile(path) => {
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
            Action::Settings => self.open_form(Form::Settings),
            Action::ToggleColorBlindMode => self.toggle_color_blind_mode(),
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
    fn toggle_recipient(&mut self, id: Option<AgentId>) {
        let Some(room) = self.room else {
            return;
        };
        let all: BTreeSet<_> = self
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
            } else if local.recipients == all {
                local.recipients.clear();
            } else {
                local.recipients = all;
            }
        }
        self.recipients_changed(room);
    }
    fn insert(&mut self, text: &str) {
        if let Some(rename) = &mut self.rename {
            rename.editor.insert(text);
            return;
        }
        if let Some(form) = &mut self.form {
            if let Some(editor) = form.editor_mut() {
                editor.insert(text);
            }
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
    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.rename.is_some() {
            match code {
                KeyCode::Esc => self.rename = None,
                KeyCode::Enter => {
                    if let Some(rename) = self.rename.take() {
                        let cmd = match rename.target {
                            RenameTarget::Room(id) => {
                                BusCommand::RenameRoom(id, rename.editor.text)
                            }
                            RenameTarget::Agent(id) => {
                                BusCommand::RenameAgent(id, rename.editor.text)
                            }
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
            return;
        }
        if self.form.is_some() {
            self.form_key(code, modifiers);
            return;
        }
        if self.recipient_menu {
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
                KeyCode::Enter | KeyCode::Char(' ') => self.toggle_recipient(
                    self.recipient_index
                        .checked_sub(1)
                        .and_then(|i| ids.get(i).copied()),
                ),
                _ => {}
            }
            return;
        }
        if self.history_search.is_some() && !self.notes_focus {
            self.search_key(code, modifiers);
            return;
        }
        if !matches!(code, KeyCode::Esc) {
            self.last_esc = None;
        }
        match (code, modifiers) {
            (KeyCode::F(2), _) => {
                if let Some(id) = self.terminal {
                    self.start_rename(RenameTarget::Agent(id));
                } else if let Some(id) = self.room {
                    self.start_rename(RenameTarget::Room(id));
                }
            }
            (KeyCode::F(3), _) => self.notes_focus = !self.notes_focus,
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
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => self.action(Action::Files),
            (KeyCode::Char('@' | '+'), modifiers)
                if !self.notes_focus && modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                self.action(if code == KeyCode::Char('@') {
                    Action::Recipients
                } else {
                    Action::Files
                });
            }
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
                        if !self.notes_focus {
                            local.composer_scroll = None;
                        }
                        let editor = if self.notes_focus {
                            &mut local.notes
                        } else {
                            &mut local.text
                        };
                        let changed = editor.key(code, modifiers);
                        if changed {
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
    fn escape(&mut self) {
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
    fn archive_draft(&mut self) {
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
    fn stash_prompt(&mut self) {
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
    fn finish_line_continue(&mut self) {
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
    fn paste_image(&mut self) {
        let Some(image) = crate::platform::read_clipboard_image() else {
            return;
        };
        let Some(room) = self.room else {
            return;
        };
        let path = std::env::temp_dir().join(format!(
            "bus-paste-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            image.extension
        ));
        if std::fs::write(&path, &image.bytes).is_err() {
            self.error = Some("Could not write the clipboard image.".into());
            return;
        }
        self.queue(
            BusCommand::AttachFile(room, path.display().to_string()),
            Effect::Files(room),
        );
    }
    fn history_entries(&self, room: RoomId) -> Vec<String> {
        let mut entries = self
            .locals
            .get(&room)
            .map(|local| local.recall.clone())
            .unwrap_or_default();
        for request in self
            .snapshot
            .state
            .requests()
            .filter(|request| request.room_id == room)
        {
            if entries.last() != Some(&request.prompt.text) {
                entries.push(request.prompt.text.clone());
            }
        }
        if let Some(text) = self
            .snapshot
            .state
            .room(room)
            .and_then(|room| room.latest_prompt.as_ref())
            .map(|prompt| prompt.text.clone())
        {
            if entries.last() != Some(&text) {
                entries.push(text);
            }
        }
        entries
    }
    fn history_or_move(&mut self, code: KeyCode) {
        let Some(room) = self.room else {
            return;
        };
        let Some(local) = self.locals.get(&room) else {
            return;
        };
        if code == KeyCode::Up && local.text.at_first_line() {
            self.history_older(room);
        } else if code == KeyCode::Down && local.text.at_last_line() {
            self.history_newer(room);
        } else if let Some(local) = self.locals.get_mut(&room) {
            local.composer_scroll = None;
            local.text.key(code, KeyModifiers::NONE);
        }
    }
    fn history_older(&mut self, room: RoomId) {
        let entries = self.history_entries(room);
        if entries.is_empty() {
            return;
        }
        let Some(local) = self.locals.get_mut(&room) else {
            return;
        };
        let index = match local.history_index {
            None => {
                local.live_draft = Some(local.text.text.clone());
                entries.len() - 1
            }
            Some(0) => 0,
            Some(index) => index - 1,
        };
        local.history_index = Some(index);
        local.text = Editor::new(entries[index].clone());
        self.text_changed(room);
    }
    fn history_newer(&mut self, room: RoomId) {
        let entries = self.history_entries(room);
        let Some(local) = self.locals.get_mut(&room) else {
            return;
        };
        match local.history_index {
            None => {}
            Some(index) if index + 1 < entries.len() => {
                local.history_index = Some(index + 1);
                local.text = Editor::new(entries[index + 1].clone());
                self.text_changed(room);
            }
            Some(_) => {
                local.history_index = None;
                let draft = local.live_draft.take().unwrap_or_default();
                local.text = Editor::new(draft);
                self.text_changed(room);
            }
        }
    }
    fn cycle_history_search(&mut self) {
        let Some(room) = self.room else {
            return;
        };
        if self.history_entries(room).is_empty() {
            return;
        }
        match &mut self.history_search {
            None => {
                let live_draft = self
                    .locals
                    .get(&room)
                    .map(|local| local.text.text.clone())
                    .unwrap_or_default();
                self.history_search = Some(HistorySearch {
                    query: String::new(),
                    selected: 0,
                    live_draft,
                });
            }
            Some(search) => search.selected = search.selected.saturating_add(1),
        }
        self.apply_search(room);
    }
    pub(super) fn filtered_history(&self, room: RoomId, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.history_entries(room)
            .into_iter()
            .rev()
            .filter(|text| query.is_empty() || text.to_lowercase().contains(&query))
            .collect()
    }
    fn apply_search(&mut self, room: RoomId) {
        let Some(search) = &self.history_search else {
            return;
        };
        let matches = self.filtered_history(room, &search.query);
        if matches.is_empty() {
            return;
        }
        let selected = search.selected % matches.len();
        let text = matches[selected].clone();
        if let Some(search) = &mut self.history_search {
            search.selected = selected;
        }
        if let Some(local) = self.locals.get_mut(&room) {
            local.text = Editor::new(text);
            local.history_index = None;
        }
        self.text_changed(room);
    }
    fn cancel_search(&mut self) {
        let Some(search) = self.history_search.take() else {
            return;
        };
        let Some(room) = self.room else {
            return;
        };
        if let Some(local) = self.locals.get_mut(&room) {
            local.text = Editor::new(search.live_draft);
            local.history_index = None;
            local.live_draft = None;
        }
        self.text_changed(room);
    }
    fn search_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let Some(room) = self.room else {
            return;
        };
        match (code, modifiers) {
            (KeyCode::Esc, _) => self.cancel_search(),
            (KeyCode::Enter, _) => self.history_search = None,
            (KeyCode::Char('r' | 'R'), KeyModifiers::CONTROL) => {
                if let Some(search) = &mut self.history_search {
                    search.selected = search.selected.saturating_add(1);
                }
                self.apply_search(room);
            }
            (KeyCode::Backspace, _) => {
                if let Some(search) = &mut self.history_search {
                    search.query.pop();
                    search.selected = 0;
                }
                self.apply_search(room);
            }
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                if let Some(search) = &mut self.history_search {
                    search.query.push(c);
                    search.selected = 0;
                }
                self.apply_search(room);
            }
            _ => {}
        }
    }
    fn form_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if code == KeyCode::Esc {
            self.action(Action::Cancel);
            return;
        }
        if matches!(self.form, Some(Form::Settings)) {
            if code == KeyCode::Enter {
                self.toggle_color_blind_mode();
            }
            return;
        }
        if matches!(self.form, Some(Form::Help { .. })) {
            match code {
                KeyCode::Enter => self.action(Action::Cancel),
                KeyCode::Up | KeyCode::Down => self.scroll_help(code == KeyCode::Down, 1),
                KeyCode::PageUp | KeyCode::PageDown => self.scroll_help(
                    code == KeyCode::PageDown,
                    usize::from(self.view.help.height.saturating_sub(1).max(1)),
                ),
                KeyCode::Home | KeyCode::End => self.scroll_help(code == KeyCode::End, usize::MAX),
                _ => {}
            }
            return;
        }
        if matches!(code, KeyCode::Up | KeyCode::Down) && !self.suggestions.entries.is_empty() {
            self.suggestions.selected = if code == KeyCode::Up {
                self.suggestions.selected.saturating_sub(1)
            } else {
                (self.suggestions.selected + 1).min(self.suggestions.entries.len() - 1)
            };
            return;
        }
        if matches!(self.form, Some(Form::Agent { field: 1, .. }))
            && matches!(
                code,
                KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Char(' ')
                    | KeyCode::Enter
            )
        {
            if let Some(Form::Agent {
                provider,
                provider_cursor,
                field,
                ..
            }) = &mut self.form
            {
                match code {
                    KeyCode::Left | KeyCode::Up => {
                        *provider_cursor = match provider_cursor {
                            Provider::Codex => Provider::Cursor,
                            Provider::ClaudeCode => Provider::Codex,
                            Provider::Cursor => Provider::ClaudeCode,
                        };
                    }
                    KeyCode::Right | KeyCode::Down => {
                        *provider_cursor = match provider_cursor {
                            Provider::Codex => Provider::ClaudeCode,
                            Provider::ClaudeCode => Provider::Cursor,
                            Provider::Cursor => Provider::Codex,
                        };
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        *provider = Some(*provider_cursor);
                        *field = 2;
                    }
                    _ => unreachable!(),
                }
            }
            self.query_paths();
            return;
        }
        if code == KeyCode::Enter && matches!(self.form, Some(Form::Agent { .. })) {
            self.add();
            return;
        }
        if matches!(code, KeyCode::Tab | KeyCode::Enter) && !self.suggestions.entries.is_empty() {
            self.complete_path();
            return;
        }
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::CONTROL) {
            self.add();
            return;
        }
        if code == KeyCode::Tab || code == KeyCode::BackTab || code == KeyCode::Enter {
            if let Some(Form::Agent { field, .. }) = &mut self.form {
                *field = if code == KeyCode::BackTab {
                    (*field + 3) % 4
                } else {
                    (*field + 1) % 4
                };
                self.query_paths();
                return;
            }
            if code == KeyCode::Enter {
                self.add();
            }
            return;
        }
        if let KeyCode::Char(c) = code {
            if !modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            {
                self.insert(&c.to_string());
                return;
            }
        }
        // Modified letters are editing chords (Alt+B/F/D, Ctrl+W) or no-ops.
        let edited = self
            .form
            .as_mut()
            .and_then(Form::editor_mut)
            .is_some_and(|editor| {
                let before = editor.cursor;
                editor.key(code, modifiers) || editor.cursor != before
            });
        if edited || !matches!(code, KeyCode::Char(_)) {
            self.query_paths();
        }
    }
    fn query_paths(&mut self) {
        self.suggestions.entries.clear();
        self.suggestions.query_id += 1;
        if let Some((input, directories_only)) = self.form.as_ref().and_then(Form::path_query) {
            let query_id = self.suggestions.query_id;
            self.queue(
                BusCommand::Suggestions {
                    query_id,
                    input,
                    directories_only,
                },
                Effect::None,
            );
        }
    }
    fn complete_path(&mut self) {
        let Some(entry) = self
            .suggestions
            .entries
            .get(self.suggestions.selected)
            .cloned()
        else {
            return;
        };
        if !entry.is_directory && matches!(self.form, Some(Form::Files(_))) {
            if let Some(room) = self.room {
                self.queue(
                    BusCommand::AttachFile(room, entry.path.display().to_string()),
                    Effect::Files(room),
                );
                self.open_room(room);
            }
            return;
        }
        if let Some(editor) = self.form.as_mut().and_then(Form::editor_mut) {
            *editor = Editor::new(format!("{}/", entry.path.display()));
        }
        // Completing a directory keeps it selected; type a prefix or press Down to browse further.
        self.suggestions.entries.clear();
        self.suggestions.query_id += 1;
    }
    fn add(&mut self) {
        if self.pending.iter().any(|p| {
            matches!(
                p.command,
                BusCommand::AddAgent(_) | BusCommand::CreateRoom(_)
            )
        }) {
            return;
        }
        let Some(form) = self.form.clone() else {
            return;
        };
        match form {
            Form::Help { .. } | Form::Settings => {}
            Form::Room(editor) => {
                self.queue(BusCommand::CreateRoom(editor.text), Effect::None);
            }
            Form::Agent {
                name,
                provider,
                cwd,
                args,
                ..
            } => {
                let mut missing = Vec::new();
                if name.text.trim().is_empty() {
                    missing.push("Name");
                }
                if provider.is_none() {
                    missing.push("Model");
                }
                if cwd.text.trim().is_empty() {
                    missing.push("PWD");
                }
                if !missing.is_empty() {
                    self.error = Some(match missing.as_slice() {
                        [field] => format!("{field} is required."),
                        [first, second] => format!("{first} and {second} are required."),
                        [first, second, third] => {
                            format!("{first}, {second}, and {third} are required.")
                        }
                        _ => unreachable!(),
                    });
                    return;
                }
                self.error = None;
                if let Some(room) = self.room {
                    self.queue(
                        BusCommand::AddAgent(AddAgent {
                            room,
                            name: name.text,
                            provider: provider.expect("validated provider"),
                            cwd: cwd.text,
                            extra_args: args.text,
                            consent_project_hooks: false,
                        }),
                        Effect::None,
                    );
                }
            }
            Form::Files(editor) => {
                if let Some(room) = self.room {
                    self.queue(
                        BusCommand::AttachFile(room, editor.text),
                        Effect::Files(room),
                    );
                    self.open_room(room);
                }
            }
            Form::Consent { mut input, .. } => {
                input.consent_project_hooks = true;
                self.queue(BusCommand::AddAgent(input), Effect::None);
            }
        }
    }
}
