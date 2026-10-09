//! Sound settings and the settings/help scroll windows.
use super::super::{forms::Form, render::SoundTarget, BusUi, Effect};
use crate::messaging::coordinator::BusCommand;

impl BusUi {
    pub(super) fn toggle_sound(&mut self, target: SoundTarget) {
        match target {
            SoundTarget::Room(room) => {
                if let Some(enabled) = self
                    .snapshot
                    .state
                    .room(room)
                    .map(|room| room.sound_enabled())
                {
                    self.queue(BusCommand::SetRoomSound(room, !enabled), Effect::None);
                }
            }
            SoundTarget::AllRooms => {
                // Like a tri-state checkbox: only all-on turns off; off or
                // mixed turns every room on.
                let enabled = self.all_rooms_sound().0 != Some(true);
                // Shown at once when there are no rooms; the coordinator's
                // saved copy and the rooms' snapshot follow.
                self.settings.room_sound.enabled = enabled;
                self.queue(BusCommand::SetAllRoomsSound(enabled), Effect::None);
            }
        }
    }

    /// Moves a sound row to the next or previous sound (Default first, then
    /// the system sounds) and plays it as a preview.
    pub(in crate::client::rooms) fn cycle_sound(&mut self, target: SoundTarget, forward: bool) {
        let current = match target {
            SoundTarget::Room(room) => match self.snapshot.state.room(room) {
                Some(room) => room.sound_name.clone(),
                None => return,
            },
            // Mixed sounds cycle from Default.
            SoundTarget::AllRooms => self.all_rooms_sound().1.flatten(),
        };
        let mut choices: Vec<Option<String>> = vec![None];
        choices.extend(self.system_sounds.iter().flatten().cloned().map(Some));
        let index = choices
            .iter()
            .position(|choice| match (choice, &current) {
                (Some(choice), Some(current)) => choice.eq_ignore_ascii_case(current),
                (choice, current) => choice.is_none() && current.is_none(),
            })
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % choices.len()
        } else {
            (index + choices.len() - 1) % choices.len()
        };
        let choice = choices.swap_remove(next);
        if let Some(config) = &self.sound_config {
            crate::platform::sound::play_named(choice.as_deref(), config);
        }
        let command = match target {
            SoundTarget::Room(room) => BusCommand::SetRoomSoundName(room, choice),
            SoundTarget::AllRooms => {
                self.settings.room_sound.name = choice.clone();
                BusCommand::SetAllRoomsSoundName(choice)
            }
        };
        self.queue(command, Effect::None);
    }

    /// A sound name as Settings shows it, noting one no longer installed.
    pub(in crate::client::rooms) fn sound_label(&self, name: Option<&str>) -> String {
        match name {
            None => crate::platform::sound::DEFAULT_SOUND_NAME.into(),
            Some(name)
                if self.system_sounds.as_ref().is_some_and(|sounds| {
                    !sounds.iter().any(|sound| sound.eq_ignore_ascii_case(name))
                }) =>
            {
                format!("{name} (missing)")
            }
            Some(name) => name.to_owned(),
        }
    }

    /// Keeps the keyboard-focused sound row inside the scrolled Settings list.
    pub(super) fn reveal_settings_field(&mut self) {
        let lines = self.sound_settings_lines();
        let Some(line) = lines.iter().position(|line| {
            matches!(line, super::super::render::SoundSettingsLine::Sound { field, .. } if *field == self.settings_field)
        }) else {
            self.settings_scroll = 0;
            return;
        };
        // Bring a group heading into view along with its first row.
        let line = match line.checked_sub(1).map(|above| &lines[above]) {
            Some(super::super::render::SoundSettingsLine::Heading(_)) => line - 1,
            _ => line,
        };
        let height = usize::from(self.view.settings_list.height.max(1));
        if line < self.settings_scroll {
            self.settings_scroll = line;
        } else if line >= self.settings_scroll + height {
            self.settings_scroll = line + 1 - height;
        }
    }

    pub(super) fn scroll_help(&mut self, forward: bool, lines: usize) {
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
}
