//! Dev-only selection and notification routing on the single writer.
use super::{mpsc, settings, BusEvent, RoomKind, Worker};

impl Worker {
    pub(super) fn refresh_error_pipe_at(&mut self, now: u64) {
        let selected = self
            .dev_enabled
            .then(|| {
                self.settings_path
                    .as_deref()
                    .and_then(|path| settings::load(path).ok())
                    .and_then(|settings| settings.error_orchestrator)
            })
            .flatten();
        let target = selected
            .as_deref()
            .and_then(|name| {
                self.state.agents().find(|agent| {
                    agent.name == name
                        && !agent.deletion_pending
                        && agent.orchestrates.is_some()
                        && self
                            .state
                            .room(agent.room_id)
                            .is_some_and(|room| room.kind == RoomKind::Master)
                })
            })
            .and_then(|agent| agent.orchestrates.map(|room| (agent.id, room)));
        let dir = self.error_log_dir();
        self.error_pipe
            .configure(&dir, target.and(selected.as_deref()));
        #[cfg(feature = "dev-tools")]
        if self.dev_enabled {
            crate::devtools::error_fixture::emit(&self.data_dir);
        }
        let Some((agent, room)) = target else {
            return;
        };
        let notices = self.error_pipe.collect(&dir, now);
        if notices.is_empty() {
            return;
        }
        let _report = crate::utils::error_pipe::suppress(true);
        let mut state = self.state.clone();
        for text in notices {
            if state
                .submit_message_with(
                    room,
                    super::Draft {
                        text,
                        files: Vec::new(),
                        recipient_ids: [agent].into(),
                    },
                    super::Author::Bus,
                    now,
                    true,
                )
                .is_err()
            {
                return;
            }
        }
        // A report-send failure is consumed. It never becomes another report or a UI error.
        let _ = self.save(state);
    }
    pub(super) fn error_orchestrators(&self) -> Vec<String> {
        self.state
            .agents()
            .filter(|agent| {
                !agent.deletion_pending
                    && agent.orchestrates.is_some()
                    && self
                        .state
                        .room(agent.room_id)
                        .is_some_and(|room| room.kind == RoomKind::Master)
            })
            .map(|agent| agent.name.clone())
            .collect()
    }

    pub(super) fn set_error_orchestrator(
        &mut self,
        name: Option<String>,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        if !self.dev_enabled {
            return Err("Error reporting requires --dev".into());
        }
        if name
            .as_ref()
            .is_some_and(|name| !self.error_orchestrators().contains(name))
        {
            return Err("Choose a current MASTER orchestrator or Off".into());
        }
        let path = self
            .settings_path
            .as_deref()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::update(path, |settings| settings.error_orchestrator = name.clone())?;
        self.error_pipe
            .configure(&self.error_log_dir(), name.as_deref());
        let _ = events.send(BusEvent::SettingsChanged(saved));
        Ok(())
    }

    pub(super) fn error_log_dir(&self) -> std::path::PathBuf {
        self.data_dir.join("herdr-config/sessions/bus")
    }
}
