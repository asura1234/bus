mod chat_search;
#[path = "dialogs/deletion.rs"]
mod deletion;
mod drafts;
#[path = "widgets/editor.rs"]
mod editor;
#[path = "dialogs/forms.rs"]
mod forms;
mod help;
mod history;
mod input;
#[path = "widgets/recipients.rs"]
mod recipients;
mod render;
mod ring;
mod selection;
#[path = "ui.rs"]
mod state;
#[path = "render/thumbnails.rs"]
mod thumbnails;
mod toast;
pub(super) use render::layout;
pub(super) use state::*;

impl crate::client::compositor::ClientShellState {
    /// `enabled` is whether this client presents graphics at all; the host
    /// terminal must also be one known to draw images (Kitty or iTerm2).
    pub(crate) fn set_bus_kitty_graphics(&mut self, enabled: bool) {
        if let Some(bus) = self.bus.as_mut() {
            bus.graphics = enabled
                .then(|| thumbnails::host_graphics_protocol(|key| std::env::var(key).ok()))
                .flatten();
            if let Some(protocol) = bus.graphics {
                bus.thumbnails.set_protocol(protocol);
            }
        }
    }

    /// The client wrote a frame, clearing the screen first when `cleared`.
    pub(crate) fn bus_frame_presented(&mut self, cleared: bool) {
        if let Some(bus) = self.bus.as_mut() {
            bus.graphics_presented(cleared);
        }
    }

    /// The client repainted every cell without drawing Bus graphics in that
    /// frame, which erases iTerm2 images; draw them again.
    pub(crate) fn bus_graphics_erased(&mut self) {
        if let Some(bus) = self.bus.as_mut() {
            bus.thumbnails.invalidate();
        }
    }

    pub(crate) fn has_bus(&self) -> bool {
        self.bus.is_some()
    }

    /// Whether the Bus view switched screens since the last call, so the
    /// client should repaint every cell instead of only the changed ones.
    pub(crate) fn take_bus_full_repaint(&mut self) -> bool {
        self.bus
            .as_mut()
            .is_some_and(|bus| std::mem::take(&mut bus.full_repaint))
    }

    pub(super) fn compute_bus_view(&mut self, cols: u16, rows: u16) {
        let cell = self.graphics_cell_size;
        if let Some(bus) = self.bus.as_mut() {
            bus.thumbnails.set_cell(bus.graphics.map(|_| cell));
            bus.compute_view(cols, rows);
        }
    }

    /// `sound` is the user's `[ui.sound]`, used only for custom sound paths; Bus
    /// rooms decide on their own whether to ring.
    pub(crate) fn start_bus(&mut self, sound: &crate::config::SoundConfig) -> Result<(), String> {
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
        // Only the client that owns the coordinator plays room sounds, so each rings once.
        bus.sound_config = Some(sound.clone());
        bus.settings_path = crate::bus::settings::path();
        if let Some(path) = &bus.settings_path {
            match crate::bus::settings::load(path) {
                Ok(settings) => bus.settings = settings,
                Err(error) => bus.error = Some(error),
            }
        }
        if let Some(room) = bus.room {
            bus.open_room(room);
        }
        if bus.seed_first_room {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bus::{model::*, runtime::*},
        input::TerminalKey,
        raw_input::RawInputEvent,
    };
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::Arc;

    #[path = "support_test.rs"]
    mod support;
    use support::*;

    #[path = "deletion_test.rs"]
    mod deletion_tests;

    #[path = "sidebar_test.rs"]
    mod sidebar_tests;

    #[path = "toasts_test.rs"]
    mod toasts_tests;

    #[path = "native_shell_test.rs"]
    mod native_shell_tests;

    #[path = "chat_search_test.rs"]
    mod chat_search_tests;

    #[path = "history_markdown_test.rs"]
    mod history_markdown_tests;

    #[path = "history_slots_test.rs"]
    mod history_slots_tests;

    #[path = "history_scroll_test.rs"]
    mod history_scroll_tests;

    #[path = "keys_test.rs"]
    mod keys_tests;

    #[path = "master_test.rs"]
    mod master_tests;

    #[path = "sound_test.rs"]
    mod sound_tests;

    include!("tests/composer_test.rs");
    include!("tests/forms_test.rs");
    include!("tests/layout_test.rs");
    include!("tests/attachments_test.rs");
    include!("tests/selection_test.rs");
}
