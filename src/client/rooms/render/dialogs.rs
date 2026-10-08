//! Forms, sound-setting rows and deletion confirmation views.
use super::super::{deletion::DeleteTarget, forms::Form, BusUi};
use super::geometry::agent_form_gap;
use super::text::{display, provider, wrap};
use super::{Action, SoundSettingsLine, SoundTarget, View};
use crate::bus::model::RoomKind;
use ratatui::layout::Rect;

impl BusUi {
    /// MASTER in its own group first, then the new-room default and every
    /// work room, in sidebar order.
    pub(in crate::client::rooms) fn sound_settings_targets(&self) -> Vec<SoundTarget> {
        let state = &self.snapshot.state;
        let master = state.master_room().map(|room| SoundTarget::Room(room.id));
        let work = state
            .rooms()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| SoundTarget::Room(room.id));
        master
            .into_iter()
            .chain(std::iter::once(SoundTarget::AllRooms))
            .chain(work)
            .collect()
    }

    pub(in crate::client::rooms) fn sound_settings_lines(&self) -> Vec<SoundSettingsLine> {
        let master = self
            .snapshot
            .state
            .master_room()
            .map(|room| SoundTarget::Room(room.id));
        let mut lines = Vec::new();
        let mut targets = self
            .sound_settings_targets()
            .into_iter()
            .enumerate()
            .map(|(index, target)| SoundSettingsLine::Sound {
                target,
                field: index + 1,
            })
            .peekable();
        if let Some(line) =
            targets.next_if(|line| matches!(line, SoundSettingsLine::Sound { target, .. } if Some(*target) == master))
        {
            lines.push(SoundSettingsLine::Heading("MASTER"));
            lines.push(line);
            lines.push(SoundSettingsLine::Empty(""));
        }
        lines.push(SoundSettingsLine::Heading("ROOMS"));
        lines.extend(targets.next());
        if targets.peek().is_none() {
            lines.push(SoundSettingsLine::Empty("No rooms yet"));
        }
        lines.extend(targets);
        lines
    }

    /// A Settings sound row's checkbox label, whether it is on (None when the
    /// rooms under All rooms differ), and its sound.
    pub(in crate::client::rooms) fn sound_row(
        &self,
        target: SoundTarget,
    ) -> Option<(String, Option<bool>, String)> {
        match target {
            SoundTarget::Room(room) => {
                let room = self.snapshot.state.room(room)?;
                Some((
                    format!("# {}", room.name),
                    Some(room.sound_enabled()),
                    self.sound_label(room.sound_name.as_deref()),
                ))
            }
            SoundTarget::AllRooms => {
                let (enabled, name) = self.all_rooms_sound();
                let sound = match name {
                    Some(name) => self.sound_label(name.as_deref()),
                    None => "Mixed".into(),
                };
                Some(("All rooms".into(), enabled, sound))
            }
        }
    }

    /// The work rooms' shared on/off state and sound, each None when the rooms
    /// differ. Without work rooms it is the saved default new rooms get.
    pub(in crate::client::rooms) fn all_rooms_sound(
        &self,
    ) -> (Option<bool>, Option<Option<String>>) {
        let mut rooms = self
            .snapshot
            .state
            .rooms()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| (room.sound_enabled(), room.sound_name.clone()));
        let Some((enabled, name)) = rooms.next() else {
            let pref = &self.settings.room_sound;
            return (Some(pref.enabled), Some(pref.name.clone()));
        };
        let (mut enabled, mut name) = (Some(enabled), Some(name));
        for (other_enabled, other_name) in rooms {
            if enabled != Some(other_enabled) {
                enabled = None;
            }
            if name.as_ref() != Some(&other_name) {
                name = None;
            }
        }
        (enabled, name)
    }
}

impl BusUi {
    pub(super) fn delete_view(&self, view: &mut View, area: Rect) {
        let Some(dialog) = &self.deletion else {
            return;
        };
        let (title, message) = match dialog.target {
            DeleteTarget::Room(id) => {
                let name = self
                    .snapshot
                    .state
                    .room(id)
                    .map_or("room", |room| room.name.as_str());
                let state = &self.snapshot.state;
                let count = state.agents().filter(|agent| agent.room_id == id).count();
                // The orchestrator lives in MASTER but exists only for this room.
                let orchestrator = state.orchestrator_of(id).map_or(String::new(), |agent| {
                    format!(" and its orchestrator \"{}\" in MASTER", agent.name)
                });
                (format!("Delete room \"{name}\"?"), format!("This will close its {count} agents{orchestrator} and permanently delete this room’s session data."))
            }
            DeleteTarget::Agent(id) => {
                let name = self
                    .snapshot
                    .state
                    .agent(id)
                    .map_or("agent", |agent| agent.name.as_str());
                (format!("Delete agent \"{name}\"?"), "This will close the agent and permanently delete its session data from this room.".into())
            }
        };
        let width = area.width.saturating_sub(2).min(66);
        let text_width = width.saturating_sub(4);
        let mut body = wrap(dialog.error.as_ref().map_or(message.as_str(), |error| {
            if error.contains("server") {
                "Bus server needs an update before deletion can finish. Session data was kept."
            } else {
                "Deletion did not finish. Session data was kept. Check the terminal before trying again."
            }
        }), text_width);
        if dialog.command_id.is_some() {
            body.push("Stopping sessions…".into());
        }
        let height = (body.len().saturating_add(6)).min(usize::from(area.height)) as u16;
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        view.dialog = rect;
        view.dialog_rows_start = view.rows.len();
        view.hits.clear();
        view.cursor = None;
        let x = rect.x + 2;
        view.row(
            Rect::new(x, rect.y + 1, text_width, 1),
            title,
            None,
            false,
            false,
        );
        for (index, line) in body
            .into_iter()
            .take(usize::from(height.saturating_sub(5)))
            .enumerate()
        {
            view.row(
                Rect::new(x, rect.y + 2 + index as u16, text_width, 1),
                line,
                None,
                false,
                true,
            );
        }
        let y = rect.bottom().saturating_sub(2);
        let ready = dialog.command_id.is_none();
        if dialog.error.is_none() {
            view.row(
                Rect::new(x, y, text_width.min(12), 1),
                "Cancel (Esc)",
                ready.then_some(Action::CancelDelete),
                false,
                !ready,
            );
        }
        view.row(
            Rect::new(x + 16, y, text_width.saturating_sub(16).min(10), 1),
            "OK (Enter)",
            ready.then_some(Action::ConfirmDelete),
            false,
            !ready,
        );
    }

    pub(super) fn form_view(&self, view: &mut View, main: Rect, form: &Form) {
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        let mut y = 3;
        match form {
            Form::Help { scroll } => {
                view.row(
                    Rect::new(x, 1, width, 1),
                    "Keyboard shortcuts",
                    None,
                    false,
                    false,
                );
                view.help = Rect::new(x, 3, width, main.height.saturating_sub(6));
                let lines = wrap(super::super::help::TEXT, width);
                view.help_max_scroll = lines.len().saturating_sub(usize::from(view.help.height));
                view.help_scroll = (*scroll).min(view.help_max_scroll);
                for (index, line) in lines
                    .into_iter()
                    .skip(view.help_scroll)
                    .take(usize::from(view.help.height))
                    .enumerate()
                {
                    view.row(
                        Rect::new(x, 3 + index as u16, width, 1),
                        line,
                        None,
                        false,
                        false,
                    );
                }
                view.row(
                    Rect::new(x, main.bottom().saturating_sub(2), width, 1),
                    "Close (Esc / Enter) · ↑↓ / PgUp/Dn scroll",
                    Some(Action::Cancel),
                    false,
                    true,
                );
                return;
            }
            Form::Settings => {
                view.row(Rect::new(x, 1, width, 1), "Settings", None, false, false);
                view.row(
                    Rect::new(x, 3, width, 1),
                    format!(
                        "[{}] Color blind mode",
                        if self.settings.color_blind_mode {
                            "x"
                        } else {
                            " "
                        }
                    ),
                    Some(Action::ToggleColorBlindMode),
                    self.settings_field == 0,
                    false,
                );
                view.lines(
                    Rect::new(x + 4, 4, width.saturating_sub(4), 3),
                    "Agent colors stay distinct and readable with red-green or blue-yellow color blindness.",
                    None,
                    true,
                );
                view.row(
                    Rect::new(x, 8, width, 1),
                    "Sound notifications",
                    None,
                    false,
                    false,
                );
                let footer = main.bottom().saturating_sub(2);
                let error = self.visible_error().map(str::to_owned);
                let list_bottom = footer.saturating_sub(if error.is_some() { 4 } else { 1 });
                view.settings_list = Rect::new(x, 10, width, list_bottom.saturating_sub(10));
                let lines = self.sound_settings_lines();
                view.settings_max_scroll = lines
                    .len()
                    .saturating_sub(usize::from(view.settings_list.height));
                let scroll = self.settings_scroll.min(view.settings_max_scroll);
                for (index, line) in lines
                    .iter()
                    .skip(scroll)
                    .take(usize::from(view.settings_list.height))
                    .enumerate()
                {
                    let rect = Rect::new(x, 10 + index as u16, width, 1);
                    match line {
                        SoundSettingsLine::Heading(text) => {
                            view.row(rect, *text, None, false, true);
                        }
                        SoundSettingsLine::Empty(text) => {
                            view.row(rect, *text, None, false, true);
                        }
                        SoundSettingsLine::Sound { target, field } => {
                            let Some((label, enabled, sound)) = self.sound_row(*target) else {
                                continue;
                            };
                            let selected = self.settings_field == *field;
                            // The row's sound sits right of its checkbox:
                            // ‹ previous · name · next ›.
                            let name = display(&sound);
                            let name_width = (unicode_width::UnicodeWidthStr::width(name.as_str())
                                as u16)
                                .min(width.saturating_sub(16));
                            let choice_width = name_width + 4;
                            let choice_x = x + width.saturating_sub(choice_width);
                            view.row(
                                Rect::new(x, rect.y, width.saturating_sub(choice_width + 1), 1),
                                format!(
                                    "[{}] {label}",
                                    match enabled {
                                        Some(true) => "x",
                                        Some(false) => " ",
                                        // All rooms while the rooms differ.
                                        None => "-",
                                    }
                                ),
                                Some(Action::ToggleSound(*target)),
                                selected,
                                false,
                            );
                            view.row(
                                Rect::new(choice_x, rect.y, 2, 1),
                                "‹",
                                Some(Action::CycleSound(*target, false)),
                                selected,
                                true,
                            );
                            view.row(
                                Rect::new(choice_x + 2, rect.y, name_width + 2, 1),
                                format!("{name} ›"),
                                Some(Action::CycleSound(*target, true)),
                                selected,
                                false,
                            );
                        }
                    }
                }
                if let Some(error) = error {
                    view.lines(
                        Rect::new(x, footer.saturating_sub(4), width, 3),
                        &error,
                        None,
                        false,
                    );
                }
                view.row(
                    Rect::new(x, footer, width, 1),
                    "Close (Esc) · ↑↓ move · Enter toggles · ←→ sound",
                    Some(Action::Cancel),
                    false,
                    true,
                );
                return;
            }
            Form::Room(editor) => {
                view.row(Rect::new(x, y, width, 1), "Room name", None, false, true);
                view.editor(Rect::new(x, y + 1, width, 1), editor, None, true);
                y += 4;
            }
            Form::Files(editor) => {
                view.row(
                    Rect::new(x, y, width, 1),
                    "Local file path",
                    None,
                    false,
                    true,
                );
                view.editor(Rect::new(x, y + 1, width, 1), editor, None, true);
                y += 3;
                view.lines(
                    Rect::new(x, y, width, 2),
                    "Choose a real local file. Paths are references; contents are not uploaded.",
                    None,
                    true,
                );
                y += 3;
            }
            Form::Agent {
                name,
                provider: selected,
                provider_cursor: _,
                cwd,
                args,
                field,
                orchestrates,
                prompt,
            } => {
                let gap = agent_form_gap(main, form);
                for (index, label, editor) in [
                    (0, "Name", Some(name)),
                    (1, "Agent", None),
                    (2, "PWD", Some(cwd)),
                    (3, "Additional launch args (optional)", Some(args)),
                ] {
                    view.row(
                        Rect::new(x, y, width, 1),
                        label,
                        Some(Action::Field(index)),
                        false,
                        true,
                    );
                    y += 1;
                    if let Some(editor) = editor {
                        view.editor(
                            Rect::new(x, y, width, 1),
                            editor,
                            Some(Action::Field(index)),
                            *field == index,
                        );
                    } else {
                        view.row(
                            Rect::new(x, y, width, 1),
                            format!("< {} >", selected.map_or("Choose model", provider)),
                            Some(Action::Field(1)),
                            *field == 1,
                            false,
                        );
                    }
                    y += gap;
                }
                if let Some(choice) = orchestrates {
                    view.row(
                        Rect::new(x, y, width, 1),
                        "Orchestrates room",
                        Some(Action::Field(super::super::forms::ORCHESTRATES_FIELD)),
                        false,
                        true,
                    );
                    y += 1;
                    let target = choice
                        .0
                        .and_then(|room| self.snapshot.state.room(room))
                        .map_or("no work room without an orchestrator", |room| {
                            room.name.as_str()
                        });
                    view.row(
                        Rect::new(x, y, width, 1),
                        format!("< {target} >"),
                        Some(Action::Orchestrates),
                        *field == super::super::forms::ORCHESTRATES_FIELD,
                        false,
                    );
                    y += gap;
                }
                if let Some(prompt) = prompt {
                    let index = super::super::forms::PROMPT_FIELD;
                    view.row(
                        Rect::new(x, y, width, 1),
                        "System prompt (Enter adds a line, Ctrl+Enter adds the agent)",
                        Some(Action::Field(index)),
                        false,
                        true,
                    );
                    y += 1;
                    // The prompt takes what the buttons and a validation error leave.
                    let below = if self.visible_error().is_some() { 7 } else { 3 };
                    let height = main.bottom().saturating_sub(y + below).clamp(1, 16);
                    view.editor(
                        Rect::new(x, y, width, height),
                        &prompt.editor,
                        Some(Action::Field(index)),
                        *field == index,
                    );
                    y += height + 1;
                }
            }
            Form::Consent { notice, .. } => {
                let text = format!(
                    "{}\n{}\n\nAdd confirms writing these Bus-owned hook entries. Bus will become ready automatically; this does not approve provider permissions.",
                    notice.message,
                    notice.path.display()
                );
                let height = wrap(&text, width).len() as u16;
                view.lines(Rect::new(x, y, width, height), &text, None, false);
                y += height + 2;
            }
        }
        view.row(
            Rect::new(x, y, 14.min(width), 1),
            "Cancel (Esc)",
            Some(Action::Cancel),
            false,
            true,
        );
        view.row(
            Rect::new(x + 16, y, 24.min(width.saturating_sub(16)), 1),
            "Add (Enter)",
            Some(Action::Add),
            false,
            false,
        );
        y += 2;
        if let Some(error) = self.visible_error() {
            view.lines(Rect::new(x, y, width, 3), error, None, false);
            y += 4;
        }
        let remaining = main.bottom().saturating_sub(y + 2) as usize;
        let start = self
            .suggestions
            .selected
            .saturating_sub(remaining.saturating_sub(1));
        for (index, suggestion) in self
            .suggestions
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .take(remaining)
        {
            view.row(
                Rect::new(x, y + (index - start) as u16, width, 1),
                format!(
                    "{}{}",
                    suggestion.path.display(),
                    if suggestion.is_directory { "/" } else { "" }
                ),
                Some(Action::Suggestion(index)),
                index == self.suggestions.selected,
                true,
            );
        }
        if let Form::Agent {
            provider_cursor,
            field: 1,
            orchestrates,
            ..
        } = form
        {
            // Below the Agent field's value row.
            let top = 6 + agent_form_gap(main, form);
            for (index, kind) in super::super::forms::provider_choices(orchestrates.is_some())
                .iter()
                .copied()
                .enumerate()
            {
                view.row(
                    Rect::new(x, top + index as u16, width, 1),
                    provider(kind),
                    Some(Action::Provider(kind)),
                    *provider_cursor == kind,
                    false,
                );
            }
        }
    }
}
