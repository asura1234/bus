//! Dev-only selection and notification routing on the single writer.
use super::{mpsc, settings, BusEvent, RoomKind, Worker};

impl Worker {
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
