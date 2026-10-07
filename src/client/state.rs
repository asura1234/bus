use super::*;

/// State tracking for the thin client.
pub(super) struct ClientState {
    /// Stateful semantic-frame encoder used when the server sends FrameData.
    pub(super) blit_encoder: render_ansi::BlitEncoder,
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
    pub(super) sound_config: crate::config::SoundConfig,
    pub(super) kitty_graphics_enabled: bool,
    pub(super) pixel_geometry_enabled: bool,
    pub(super) pixel_geometry_exact: bool,
    pub(super) redraw_on_focus_gained: bool,
    pub(super) repaint_pending: bool,
    pub(super) draw_host_cursor: bool,
    pub(super) detached_process_children: Vec<std::process::Child>,
    pub(super) shell: Option<shell::ClientShellState>,
}

impl ClientState {
    pub(super) fn request_repaint(&mut self) {
        self.repaint_pending = true;
    }

    pub(super) fn present_graphics(&mut self, graphics: &[u8]) {
        if graphics.is_empty() || !self.kitty_graphics_enabled {
            return;
        }
        let mut stdout = io::stdout();
        let _ = write_encoded_frame_with_graphics(&mut stdout, &[], graphics);
        let _ = stdout.flush();
    }

    pub(super) fn present_surface_patch(
        &mut self,
        patch: shell::ClientComposedSurfacePatch,
    ) -> io::Result<bool> {
        if self.repaint_pending {
            crate::render_prof::event("client_surface_patch.fallback.repaint");
            return Ok(false);
        }
        let rows = if self.draw_host_cursor {
            let Some(rows) = self
                .blit_encoder
                .patch_rows_with_drawn_cursor(&patch.rows, patch.cursor.as_ref())
            else {
                crate::render_prof::event("client_surface_patch.fallback.drawn_cursor");
                return Ok(false);
            };
            rows
        } else {
            patch.rows
        };
        let encode_started = crate::render_prof::timer();
        let Some(encoded) =
            self.blit_encoder
                .encode_patch(&rows, patch.cursor.clone(), self.draw_host_cursor)
        else {
            crate::render_prof::event("client_surface_patch.fallback.encode");
            return Ok(false);
        };
        crate::render_prof::duration_since("client_surface_patch.encode", encode_started);
        let write_started = crate::render_prof::timer();
        let mut stdout = io::stdout();
        stdout.write_all(&encoded.bytes)?;
        stdout.flush()?;
        crate::render_prof::duration_since("client_surface_patch.write", write_started);
        let committed = self.blit_encoder.commit_patch(&rows, patch.cursor, encoded);
        crate::render_prof::event(if committed {
            "client_surface_patch.success"
        } else {
            "client_surface_patch.fallback.commit"
        });
        Ok(committed)
    }

    pub(super) fn present_frame(&mut self, frame_data: FrameData) {
        if self
            .shell
            .as_mut()
            .is_some_and(|shell| shell.take_bus_full_repaint())
        {
            self.request_repaint();
        }
        let frame_data = if self.draw_host_cursor {
            render_ansi::frame_with_drawn_cursor(frame_data)
        } else {
            frame_data
        };
        let cleared = self.blit_encoder.clears_before(&frame_data);
        let encoded = if self.draw_host_cursor {
            self.blit_encoder
                .encode_with_suppressed_visible_cursor(&frame_data, self.repaint_pending)
        } else {
            self.blit_encoder.encode(&frame_data, self.repaint_pending)
        };
        let mut stdout = io::stdout();
        let graphics = if self.kitty_graphics_enabled {
            frame_data.graphics.as_slice()
        } else {
            &[]
        };
        if self.repaint_pending && graphics.is_empty() {
            if let Some(shell) = self.shell.as_mut() {
                shell.bus_graphics_erased();
            }
        }
        let _ = write_encoded_frame_with_graphics(&mut stdout, &encoded.bytes, graphics);
        let _ = stdout.flush();
        if let Some(shell) = self.shell.as_mut() {
            shell.bus_frame_presented(cleared);
        }
        self.blit_encoder.commit(frame_data, encoded);
        self.repaint_pending = false;
    }
}
