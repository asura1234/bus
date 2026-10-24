//! The Settings screen: color blind mode, the sound rows, the compaction
//! limit, disk thresholds, sidebar width and dev performance
//! overlay.
use super::super::BusUi;
use super::text::display;
use super::{Action, SoundSettingsLine, SoundTarget, View};
use crate::messaging::model::RoomKind;
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
        let mut lines = vec![
            SoundSettingsLine::Heading("YOU"),
            SoundSettingsLine::Sound {
                field: self.developer_sound_field(),
                target: SoundTarget::Developer,
            },
            SoundSettingsLine::Empty(""),
        ];
        let mut targets = self
            .sound_settings_targets()
            .into_iter()
            .enumerate()
            .map(|(index, target)| SoundSettingsLine::Sound {
                target,
                field: self.room_sound_field(index),
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
        lines.push(SoundSettingsLine::Empty(""));
        lines.push(SoundSettingsLine::Heading("AGENTS"));
        lines.push(SoundSettingsLine::Stepper {
            field: self.compaction_field(),
            label: "Max compactions per agent (1–100)",
            value: self.settings.max_compactions_per_agent.to_string(),
            adjust: Action::AdjustCompactionLimit,
        });
        lines.push(SoundSettingsLine::Empty(""));
        lines.push(SoundSettingsLine::Heading("DISK"));
        lines.push(SoundSettingsLine::Stepper {
            field: self.disk_warning_field(),
            label: "Disk warning (GB free)",
            value: self.settings.disk_warning_gb.to_string(),
            adjust: Action::AdjustDiskWarningGb,
        });
        lines.push(SoundSettingsLine::Stepper {
            field: self.disk_critical_field(),
            label: "Disk critical (GB free)",
            value: self.settings.disk_critical_gb.to_string(),
            adjust: Action::AdjustDiskCriticalGb,
        });
        lines.push(SoundSettingsLine::Empty(""));
        lines.push(SoundSettingsLine::Heading("SIDEBAR"));
        lines.push(SoundSettingsLine::Stepper {
            field: self.sidebar_width_field(),
            label: "Sidebar width (20–60)",
            value: self.settings.sidebar_width.to_string(),
            adjust: Action::AdjustSidebarWidth,
        });
        lines.push(SoundSettingsLine::Empty(""));
        lines.push(SoundSettingsLine::Heading("NOTIFICATIONS"));
        lines.push(SoundSettingsLine::Feishu {
            field: self.feishu_field(),
        });
        lines.push(SoundSettingsLine::FeishuRecipient {
            field: self.feishu_field() + 1,
        });
        lines.push(SoundSettingsLine::FeishuCli {
            field: self.feishu_field() + 2,
        });
        if let Some(field) = self.perf_overlay_field() {
            lines.push(SoundSettingsLine::Empty(""));
            lines.push(SoundSettingsLine::Heading("DEVELOPER"));
            lines.push(SoundSettingsLine::PerfOverlay { field });
        }
        lines
    }

    /// A Settings sound row's checkbox label, whether it is on (None when the
    /// rooms under All rooms differ), and its sound.
    pub(in crate::client::rooms) fn sound_row(
        &self,
        target: SoundTarget,
    ) -> Option<(String, Option<bool>, String)> {
        match target {
            SoundTarget::Developer => Some((
                "You".into(),
                Some(self.settings.developer_sound_enabled),
                self.settings.developer_sound.as_deref().map_or_else(
                    || "System default".into(),
                    |name| self.sound_label(Some(name)),
                ),
            )),
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
    pub(super) fn settings_view(&self, view: &mut View, main: Rect) {
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
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
            self.sound_settings_row(view, rect, line);
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
            "Close (Esc) · ↑↓ move · Enter changes · ←→ change",
            Some(Action::Cancel),
            false,
            true,
        );
    }

    fn sound_settings_row(&self, view: &mut View, rect: Rect, line: &SoundSettingsLine) {
        match line {
            SoundSettingsLine::FeishuRecipient { field } => {
                view.row(
                    rect,
                    format!(
                        "Recipient open_id: {}",
                        display(&self.settings.feishu_recipient)
                    ),
                    Some(Action::EditFeishuRecipient),
                    self.settings_field == *field,
                    false,
                );
            }
            SoundSettingsLine::FeishuCli { field } => {
                view.row(
                    rect,
                    format!(
                        "lark-cli path: {}",
                        display(self.settings.feishu_cli_path.as_deref().unwrap_or("PATH"))
                    ),
                    Some(Action::EditFeishuCli),
                    self.settings_field == *field,
                    false,
                );
            }
            SoundSettingsLine::Feishu { field } => {
                self.feishu_settings_row(view, rect, *field);
            }
            SoundSettingsLine::Heading(text) => {
                view.row(rect, *text, None, false, true);
            }
            SoundSettingsLine::Empty(text) => {
                view.row(rect, *text, None, false, true);
            }
            SoundSettingsLine::Stepper {
                field,
                label,
                value,
                adjust,
            } => self.stepper_settings_row(view, rect, *field, label, value, *adjust),
            SoundSettingsLine::PerfOverlay { field } => {
                let on = crate::client::perf::OVERLAY.is_on();
                view.row(
                    rect,
                    format!(
                        "[{}] Performance overlay (CPU, memory, FPS)",
                        if on { "x" } else { " " }
                    ),
                    Some(Action::TogglePerfOverlay),
                    self.settings_field == *field,
                    false,
                );
            }
            SoundSettingsLine::Sound { target, field } => {
                self.sound_pref_row(view, rect, *field, *target)
            }
        }
    }

    fn sound_pref_row(&self, view: &mut View, rect: Rect, field: usize, target: SoundTarget) {
        let Some((label, enabled, sound)) = self.sound_row(target) else {
            return;
        };
        let selected = self.settings_field == field;
        // The row's sound sits right of its checkbox:
        // ‹ previous · name · next ›.
        let name = display(&sound);
        let name_width = (unicode_width::UnicodeWidthStr::width(name.as_str()) as u16)
            .min(rect.width.saturating_sub(16));
        let choice_width = name_width + 4;
        let choice_x = rect.x + rect.width.saturating_sub(choice_width);
        view.row(
            Rect::new(
                rect.x,
                rect.y,
                rect.width.saturating_sub(choice_width + 1),
                1,
            ),
            format!(
                "[{}] {label}",
                match enabled {
                    Some(true) => "x",
                    Some(false) => " ",
                    // All rooms while the rooms differ.
                    None => "-",
                }
            ),
            Some(Action::ToggleSound(target)),
            selected,
            false,
        );
        view.row(
            Rect::new(choice_x, rect.y, 2, 1),
            "‹",
            Some(Action::CycleSound(target, false)),
            selected,
            true,
        );
        view.row(
            Rect::new(choice_x + 2, rect.y, name_width + 2, 1),
            format!("{name} ›"),
            Some(Action::CycleSound(target, true)),
            selected,
            false,
        );
    }

    fn feishu_settings_row(&self, view: &mut View, rect: Rect, field: usize) {
        let suffix = if self.feishu_status.pending {
            " (sending…)".into()
        } else if self.feishu_status.queued > 0 {
            format!(
                " ({} waiting{})",
                self.feishu_status.queued,
                if self.feishu_status.rate_limited {
                    "; rate limited"
                } else {
                    ""
                }
            )
        } else {
            String::new()
        };
        view.row(
            rect,
            format!("Feishu notifications (always on){}", suffix),
            Some(Action::ToggleFeishuNotifications),
            self.settings_field == field,
            false,
        );
    }

    /// A Settings stepper: its label, then `‹ value ›` at the right edge
    /// where each arrow is a click target for `adjust(false)` / `adjust(true)`.
    fn stepper_settings_row(
        &self,
        view: &mut View,
        rect: Rect,
        field: usize,
        label: &str,
        value: &str,
        adjust: fn(bool) -> Action,
    ) {
        let selected = self.settings_field == field;
        let label_width = unicode_width::UnicodeWidthStr::width(label);
        let reserved = (label_width + 1).min(usize::from(rect.width / 2));
        let available = usize::from(rect.width).saturating_sub(reserved);
        let value = crate::utils::text::width::truncate_end(value, available.saturating_sub(4));
        let choice_width =
            (unicode_width::UnicodeWidthStr::width(value.as_str()) + 4).min(available) as u16;
        let previous_width = choice_width.min(2);
        let choice_x = rect.x + rect.width.saturating_sub(choice_width);
        view.row(
            Rect::new(
                rect.x,
                rect.y,
                rect.width.saturating_sub(choice_width + 1),
                1,
            ),
            label,
            None,
            selected,
            false,
        );
        view.row(
            Rect::new(choice_x, rect.y, previous_width, 1),
            "‹",
            Some(adjust(false)),
            selected,
            true,
        );
        view.row(
            Rect::new(
                choice_x + previous_width,
                rect.y,
                choice_width - previous_width,
                1,
            ),
            format!("{value} ›"),
            Some(adjust(true)),
            selected,
            false,
        );
    }
}
