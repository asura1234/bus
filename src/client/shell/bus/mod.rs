mod deletion;
mod editor;
mod forms;
mod help;
mod history;
mod input;
mod recipients;
mod render;
mod selection;
mod state;
#[cfg(test)]
mod tests;
pub(super) use render::layout;
pub(super) use state::*;

impl super::ClientShellState {
    pub(crate) fn start_bus(&mut self) -> Result<(), String> {
        let Some(root) = crate::bus::entry::data_dir() else {
            return Ok(());
        };
        let handle = crate::bus::runtime::BusHandle::start(
            root,
            crate::api::client::ConnectionTarget::LocalSession(Some(
                crate::bus::runtime::DEFAULT_SESSION.into(),
            )),
        )?;
        let snapshot = handle.snapshot().ok_or("Bus initial state unavailable")?;
        let mut bus = BusUi::new(snapshot);
        bus.handle = Some(handle);
        if let Some(room) = bus.room {
            bus.open_room(room);
        } else if bus.seed_first_room {
            bus.queue(
                crate::bus::runtime::BusCommand::CreateRoom("bus".into()),
                Effect::None,
            );
        }
        bus.tick();
        if std::env::var_os("BUS_DEV_EXISTING_SERVER").is_some() {
            bus.error = Some(crate::bus::diagnostics::EXISTING_SERVER_NOTICE.into());
            tracing::warn!(
                event = "bus.dev.existing_server",
                "{}",
                crate::bus::diagnostics::EXISTING_SERVER_NOTICE
            );
        }
        self.overlay = None;
        self.bus = Some(bus);
        Ok(())
    }
    pub(crate) fn tick_bus(&mut self) -> bool {
        self.bus.as_mut().is_some_and(BusUi::tick)
    }
    pub(crate) fn bus_exit_ready(&self) -> bool {
        self.bus.as_ref().is_some_and(|bus| bus.exit_ready)
    }
    pub(crate) fn edit_bus_composer(&mut self) -> Result<(), String> {
        let bus = self
            .bus
            .as_mut()
            .ok_or_else(|| "Bus is not open".to_string())?;
        match bus.edit_in_external_editor() {
            Ok(()) => Ok(()),
            Err(error) => {
                bus.error = Some(error.clone());
                Err(error)
            }
        }
    }
    /// The native terminal owns the pane area once its target pane is on a retained surface.
    /// Exact snapshot/surface pairing is a presentation concern: agent title and status updates
    /// advance the projection several times a second, and `compose` holds the last frame until the
    /// pair catches up. Requiring the pair here flashed the opening placeholder and dropped input.
    pub(super) fn bus_terminal_ready(&self) -> bool {
        let Some(bus) = &self.bus else {
            return false;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        self.pane_surface.as_ref().is_some_and(|surface| {
            surface
                .panes
                .iter()
                .any(|p| Some(p.pane_id.as_str()) == snapshot.focused_pane_id.as_deref())
        }) && bus.terminal_ready(snapshot.focused_pane_id.as_deref())
    }
}
