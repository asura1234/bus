//! Sound settings and the settings/help scroll windows.
use super::super::{forms::Form, render::SoundTarget, BusUi, Effect};
use crate::messaging::coordinator::BusCommand;

impl BusUi {
    pub(super) fn open_feishu_setting(&mut self, cli: bool) {
        let text = if cli {
            self.settings.feishu_cli_path.clone().unwrap_or_default()
        } else {
            self.settings.feishu_recipient.clone()
        };
        self.open_form(Form::FeishuSetting {
            cli,
            editor: super::super::editor::Editor::new(text),
        });
    }

    pub(super) fn adjust_compaction_limit(&mut self, forward: bool) {
        use crate::messaging::prefs::settings::{
            MAX_COMPACTIONS_PER_AGENT, MIN_COMPACTIONS_PER_AGENT,
        };
        self.settings_field = self.compaction_field();
        let current = self.settings.max_compactions_per_agent;
        let next = if forward {
            current.saturating_add(1).min(MAX_COMPACTIONS_PER_AGENT)
        } else {
            current.saturating_sub(1).max(MIN_COMPACTIONS_PER_AGENT)
        };
        if next != current {
            self.settings.max_compactions_per_agent = next;
            self.queue(BusCommand::SetMaxCompactionsPerAgent(next), Effect::None);
        }
    }

    pub(super) fn adjust_idle_agents_alert(&mut self, up: bool) {
        use crate::messaging::prefs::settings::{MAX_IDLE_AGENTS_ALERT, MIN_IDLE_AGENTS_ALERT};
        self.settings_field = self.idle_agents_field();
        let current = self.settings.idle_agents_alert;
        let next = if up {
            current.saturating_add(1).min(MAX_IDLE_AGENTS_ALERT)
        } else {
            current.saturating_sub(1).max(MIN_IDLE_AGENTS_ALERT)
        };
        if next != current {
            self.settings.idle_agents_alert = next;
            self.queue(BusCommand::SetIdleAgentsAlert(next), Effect::None);
        }
    }

    pub(super) fn adjust_disk_threshold(&mut self, warning: bool, up: bool) {
        use crate::messaging::prefs::settings::{MAX_DISK_FREE_GB, MIN_DISK_FREE_GB};
        let (field, current, min, max) = if warning {
            (
                self.disk_warning_field(),
                self.settings.disk_warning_gb,
                self.settings.disk_critical_gb + 1,
                MAX_DISK_FREE_GB,
            )
        } else {
            (
                self.disk_critical_field(),
                self.settings.disk_critical_gb,
                MIN_DISK_FREE_GB,
                self.settings.disk_warning_gb - 1,
            )
        };
        self.settings_field = field;
        let next = if up {
            current.saturating_add(1).min(max)
        } else {
            current.saturating_sub(1).max(min)
        };
        if next == current {
            return;
        }
        let command = if warning {
            self.settings.disk_warning_gb = next;
            BusCommand::SetDiskWarningGb(next)
        } else {
            self.settings.disk_critical_gb = next;
            BusCommand::SetDiskCriticalGb(next)
        };
        self.queue(command, Effect::None);
    }

    /// Shown at the new width on the next frame; the coordinator saves it
    /// and every Bus session reads it back from `SettingsChanged`.
    pub(super) fn adjust_sidebar_width(&mut self, wider: bool) {
        use crate::messaging::prefs::settings::clamp_sidebar_width;
        self.settings_field = self.sidebar_width_field();
        let current = self.settings.sidebar_width;
        let next = clamp_sidebar_width(if wider {
            current.saturating_add(1)
        } else {
            current.saturating_sub(1)
        });
        if next != current {
            self.settings.sidebar_width = next;
            self.queue(BusCommand::SetSidebarWidth(next), Effect::None);
        }
    }

    pub(super) fn toggle_sound(&mut self, target: SoundTarget) {
        match target {
            SoundTarget::Developer => {
                let enabled = !self.settings.developer_sound_enabled;
                self.settings.developer_sound_enabled = enabled;
                self.queue(BusCommand::SetDeveloperSoundEnabled(enabled), Effect::None);
            }
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
            SoundTarget::Developer => self.settings.developer_sound.clone(),
            SoundTarget::Room(room) => match self.snapshot.state.room(room) {
                Some(room) => room.sound_name.clone(),
                None => return,
            },
            // Mixed sounds cycle from Default.
            SoundTarget::AllRooms => self.all_rooms_sound().1.flatten(),
        };
        let choice = next_sound_choice(
            current.as_deref(),
            self.system_sounds.as_deref().unwrap_or_default(),
            forward,
        );
        if let Some(config) = &self.sound_config {
            if target == SoundTarget::Developer {
                crate::platform::sound::play_developer(choice.as_deref());
            } else {
                crate::platform::sound::play_named(choice.as_deref(), config);
            }
        }
        let command = match target {
            SoundTarget::Developer => {
                self.settings.developer_sound = choice.clone();
                BusCommand::SetDeveloperSoundName(choice)
            }
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
            matches!(line,
                super::super::render::SoundSettingsLine::Sound { field, .. }
                | super::super::render::SoundSettingsLine::Stepper { field, .. }
                | super::super::render::SoundSettingsLine::PerfOverlay { field }
                | super::super::render::SoundSettingsLine::ErrorOrchestrator { field }
                | super::super::render::SoundSettingsLine::FeishuRecipient { field }
                | super::super::render::SoundSettingsLine::FeishuCli { field }
                | super::super::render::SoundSettingsLine::Feishu { field }
                if *field == self.settings_field)
        }) else {
            self.settings_scroll = 0;
            return;
        };
        // Bring a group heading into view along with its first row.
        let heading = match line.checked_sub(1).map(|above| &lines[above]) {
            Some(super::super::render::SoundSettingsLine::Heading(_)) => line - 1,
            _ => line,
        };
        let height = usize::from(self.view.settings_list.height.max(1));
        if heading < self.settings_scroll {
            self.settings_scroll = heading;
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

fn next_sound_choice(current: Option<&str>, sounds: &[String], forward: bool) -> Option<String> {
    let index = current
        .and_then(|current| {
            sounds
                .iter()
                .position(|sound| sound.eq_ignore_ascii_case(current))
        })
        .map_or(0, |index| index + 1);
    let len = sounds.len() + 1;
    let next = if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    };
    next.checked_sub(1).map(|index| sounds[index].clone())
}

impl BusUi {
    pub(in crate::client::rooms) fn error_orchestrator_choices(
        &self,
    ) -> Vec<(Option<crate::messaging::model::AgentId>, String)> {
        let state = &self.snapshot.state;
        std::iter::once((None, "Off".to_owned()))
            .chain(
                state
                    .agents()
                    .filter(|agent| {
                        !agent.deletion_pending
                            && agent.orchestrates.is_some()
                            && state.room(agent.room_id).is_some_and(|room| {
                                room.kind == crate::messaging::model::RoomKind::Master
                            })
                    })
                    .map(|agent| (Some(agent.id), agent.name.clone())),
            )
            .collect()
    }
    pub(super) fn open_error_orchestrator(&mut self) {
        let Some(field) = self.error_orchestrator_field() else {
            return;
        };
        self.settings_field = field;
        self.reveal_settings_field();
        let selected = self
            .error_orchestrator_choices()
            .iter()
            .position(|(id, name)| {
                id.is_some() && Some(name.as_str()) == self.settings.error_orchestrator.as_deref()
            })
            .unwrap_or(0);
        self.form = Some(Form::ErrorOrchestrator { selected });
    }
    pub(super) fn choose_error_orchestrator(
        &mut self,
        id: Option<crate::messaging::model::AgentId>,
    ) {
        if !self.dev_settings {
            return;
        }
        let Some((_, name)) = self
            .error_orchestrator_choices()
            .into_iter()
            .find(|(choice, _)| *choice == id)
        else {
            return;
        };
        let name = id.map(|_| name);
        self.settings.error_orchestrator = name.clone();
        self.queue(BusCommand::SetErrorOrchestrator(name), Effect::None);
        self.form = Some(Form::Settings);
    }
    pub(super) fn error_orchestrator_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;
        let choices = self.error_orchestrator_choices();
        if let Some(Form::ErrorOrchestrator { selected }) = &mut self.form {
            *selected = (*selected).min(choices.len() - 1);
            match code {
                KeyCode::Up | KeyCode::Left => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Right => {
                    *selected = (*selected + 1).min(choices.len() - 1)
                }
                KeyCode::Home => *selected = 0,
                KeyCode::End => *selected = choices.len() - 1,
                KeyCode::Enter => {
                    let id = choices[*selected].0;
                    self.choose_error_orchestrator(id);
                }
                _ => {}
            }
        }
    }
}
