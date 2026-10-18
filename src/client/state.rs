use crate::protocol::wire::FrameData;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, AtomicU16},
        Arc,
    },
};
use tracing::warn;

/// State tracking for the thin client.
pub(super) struct ClientState {
    pub(super) output: super::host_terminal::output::TerminalOutput,
    pub(super) mouse_capture_active: bool,
    pub(super) endpoint_mouse_capture_requested: bool,
    pub(super) endpoint_sgr_pixels_requested: bool,
    /// Remains set until the reader consumes the final reply from a 256-color host query.
    pub(super) host_palette_query_pending: Arc<AtomicBool>,
    /// One-based index of the latest reply consumed from the active host palette query.
    pub(super) host_palette_query_progress: Arc<AtomicU16>,
    pub(super) shell_mouse_capture_preference: bool,
    pub(super) pane_keyboard_report_all: bool,
    pub(super) keyboard_report_all_active: bool,
    pub(super) reported_size: (u16, u16),
    pub(super) reported_cell_size: (u32, u32),
    pub(super) sound_config: crate::utils::config::SoundConfig,
    pub(super) kitty_graphics_enabled: bool,
    pub(super) pixel_geometry_enabled: bool,
    pub(super) pixel_geometry_exact: bool,
    pub(super) redraw_on_focus_gained: bool,
    pub(super) repaint_pending: bool,
    pub(super) draw_host_cursor: bool,
    pub(super) detached_process_children: Vec<std::process::Child>,
    pub(super) shell: Option<crate::client::compositor::ClientShellState>,
}

impl ClientState {
    pub(super) fn request_repaint(&mut self) {
        self.repaint_pending = true;
    }

    pub(super) fn check_output(&mut self) -> io::Result<()> {
        match self.output.feedback() {
            Ok(Some(cleared)) => {
                if let Some(shell) = self.shell.as_mut() {
                    shell.bus_frame_presented(cleared);
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                self.request_repaint();
                Err(error)
            }
        }
    }

    pub(super) fn present_graphics(&mut self, graphics: &[u8]) {
        if !graphics.is_empty() && self.kitty_graphics_enabled {
            let _ = self.output.graphics(graphics);
        }
    }

    pub(super) fn present_surface_patch(
        &mut self,
        patch: crate::client::compositor::ClientComposedSurfacePatch,
    ) -> io::Result<bool> {
        if self.repaint_pending || super::host_terminal::output::busy() {
            return Ok(false);
        }
        self.output
            .patch(&patch.rows, patch.cursor, self.draw_host_cursor)
    }

    pub(super) fn present_frame(&mut self, mut frame_data: FrameData) {
        super::perf::UI_LOOP.relabel("present");
        let _present = super::perf::PRESENT.start();
        super::perf::OVERLAY.note_frame();
        if self
            .shell
            .as_mut()
            .is_some_and(|shell| shell.take_bus_full_repaint())
        {
            self.request_repaint();
        }
        if !self.kitty_graphics_enabled {
            frame_data.graphics.clear();
        }
        if self.repaint_pending && frame_data.graphics.is_empty() {
            if let Some(shell) = self.shell.as_mut() {
                shell.bus_graphics_erased();
            }
        }
        if let Err(error) =
            self.output
                .frame(frame_data, self.repaint_pending, self.draw_host_cursor)
        {
            warn!(%error, "failed to queue frame");
            self.request_repaint();
        } else {
            self.repaint_pending = false;
        }
    }
}
