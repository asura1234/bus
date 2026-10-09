use super::{
    blit_pane_surface, pane_surface_topology_signature, render, Buffer, ClientShellState, PaneHit,
    PaneSplitHit, ShellHitMap, Style,
};
use crate::protocol::wire::FrameData;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, Widget};

impl ClientShellState {
    fn compose_unavailable(&mut self, cols: u16, rows: u16) -> FrameData {
        let layout = self.layout(cols, rows);
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, rows));
        buffer.set_style(
            buffer.area,
            Style::default()
                .fg(self.config.palette.text)
                .bg(self.config.palette.panel_bg),
        );
        self.hits = ShellHitMap::default();
        let message = "Connecting to Bus.".to_owned();
        let message_area = Rect::new(layout.pane_surface.x, 0, layout.pane_surface.width, 1);
        render::put_text(
            &mut buffer,
            message_area.x,
            message_area.y,
            message_area.width,
            &message,
            Style::default().fg(self.config.palette.overlay0),
        );
        FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[])
    }

    pub(crate) fn compose(&mut self, cols: u16, rows: u16) -> Option<FrameData> {
        self.last_composed_size = Some((cols, rows));
        let bus_ready = self.bus_terminal_ready();
        self.compute_bus_view(cols, rows);
        let newly_opened = self
            .bus
            .as_ref()
            .and_then(|bus| bus.terminal_pane())
            .is_some_and(|target| self.presented_bus_terminal.as_deref() != Some(target));
        let awaiting_pair = self.pending_pane_surface.is_some()
            || self
                .snapshot
                .as_deref()
                .zip(self.pane_surface.as_ref())
                .is_none_or(|(snapshot, surface)| snapshot.revision != surface.projection_revision);
        if !bus_ready || (newly_opened && awaiting_pair) {
            if let Some(frame) = self.compose_bus_terminal_preview(cols, rows) {
                return Some(frame);
            }
        }
        if let Some(bus) = self.bus.as_mut() {
            if !bus_ready {
                self.presented_bus_terminal = None;
                self.hits = ShellHitMap::default();
                let mut buffer = Buffer::empty(Rect::new(0, 0, cols, rows));
                bus.render(&mut buffer);
                let mut frame =
                    FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, bus.cursor(), &[]);
                frame.graphics = bus.thumbnail_graphics();
                return Some(frame);
            }
        }
        if self.snapshot.is_none() || self.pane_surface.is_none() {
            return Some(self.compose_unavailable(cols, rows));
        }
        let snapshot = self.snapshot.as_deref()?;
        // A one-step successor is retained separately until its exact snapshot arrives; do not
        // keep composing the now-superseded current pair while it is pending.
        if self.pending_pane_surface.is_some() {
            return None;
        }
        let surface = self.pane_surface.as_ref()?;
        if snapshot.revision != surface.projection_revision {
            return None;
        }
        let layout = self.layout(cols, rows);
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, rows));
        if let Some(bus) = self.bus.as_ref() {
            bus.render(&mut buffer);
        }
        Clear.render(layout.pane_surface, &mut buffer);
        self.hits = ShellHitMap::default();
        self.hits.panes = surface
            .panes
            .iter()
            .map(|pane| project_pane_hit(pane, layout.pane_surface))
            .collect();
        let topology_signature = pane_surface_topology_signature(surface);
        self.hits.pane_splits = surface
            .splits
            .iter()
            .map(|split| project_split_hit(split, layout.pane_surface, topology_signature))
            .collect();
        if !self.config.mouse_capture {
            self.hits.pane_splits.clear();
        }
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        blit_pane_surface(&mut frame, &surface.frame, layout.pane_surface);
        self.compose_overlays(&mut frame, layout.pane_surface, cols, rows)?;
        self.compose_graphics(&mut frame, layout);
        if let Some(bus) = self.bus.as_mut() {
            frame.graphics.extend(bus.thumbnail_graphics());
            self.presented_bus_terminal = bus.terminal_pane().map(str::to_owned);
        }
        Some(frame)
    }

    fn compose_overlays(
        &self,
        frame: &mut FrameData,
        pane_area: Rect,
        cols: u16,
        rows: u16,
    ) -> Option<()> {
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| selection.is_visible())
        {
            let cursor = frame.cursor.clone();
            let mut composed = frame.to_ratatui_buffer()?;
            for hit in &self.hits.panes {
                crate::utils::render::widgets::selection::render_selection_highlight(
                    self.selection.as_ref(),
                    &mut composed,
                    &hit.pane_id,
                    hit.inner_rect,
                    hit.scroll,
                    &self.config.palette,
                    crate::utils::theme::color::TerminalTheme::default(),
                );
            }
            frame.replace_from_ratatui_buffer_preserving_effects(&composed, cursor);
        }
        let has_config_diagnostic = self.config_diagnostic.is_some();
        if has_config_diagnostic {
            let cursor = frame.cursor.clone();
            let mut composed = frame.to_ratatui_buffer()?;
            if let Some(diagnostic) = self.config_diagnostic.as_deref() {
                let diagnostic_area = Rect::new(0, 0, cols, rows);
                crate::utils::render::diagnostic::render_config_diagnostic_buffer(
                    &mut composed,
                    diagnostic_area,
                    diagnostic,
                    &self.config.palette,
                );
            }
            frame.replace_from_ratatui_buffer_preserving_effects(&composed, cursor);
        }
        if let Some(feedback) = self.copy_feedback.as_ref() {
            let cursor = frame.cursor.clone();
            let mut composed = frame.to_ratatui_buffer()?;
            let base_offset = u16::from(has_config_diagnostic);
            let feedback_area = pane_area;
            let offset = crate::utils::render::widgets::copy_feedback_offset_for_toast(
                feedback_area,
                feedback,
                base_offset,
                self.config.clipboard_toast_position,
                self.hits.notification_toast,
            );
            crate::utils::render::feedback::render_copy_feedback_buffer(
                &mut composed,
                feedback_area,
                feedback,
                offset,
                self.config.clipboard_toast_position,
                &self.config.palette,
            );
            frame.replace_from_ratatui_buffer_preserving_effects(&composed, cursor);
        }
        Some(())
    }
}

fn project_pane_hit(pane: &crate::protocol::wire::PaneSurfacePane, area: Rect) -> PaneHit {
    PaneHit {
        rect: Rect::new(
            area.x.saturating_add(pane.rect.x),
            area.y.saturating_add(pane.rect.y),
            pane.rect.width,
            pane.rect.height,
        ),
        inner_rect: Rect::new(
            area.x.saturating_add(pane.inner_rect.x),
            area.y.saturating_add(pane.inner_rect.y),
            pane.inner_rect.width,
            pane.inner_rect.height,
        ),
        scrollbar_rect: pane.scrollbar_rect.map(|rect| {
            Rect::new(
                area.x.saturating_add(rect.x),
                area.y.saturating_add(rect.y),
                rect.width,
                rect.height,
            )
        }),
        scroll: pane
            .scroll
            .map(|metrics| crate::utils::render::widgets::ScrollMetrics {
                offset_from_bottom: usize::try_from(metrics.offset_from_bottom)
                    .unwrap_or(usize::MAX),
                max_offset_from_bottom: usize::try_from(metrics.max_offset_from_bottom)
                    .unwrap_or(usize::MAX),
                viewport_rows: usize::try_from(metrics.viewport_rows).unwrap_or(usize::MAX),
            }),
        pane_id: pane.pane_id.clone(),
        mouse_reporting: pane.mouse_reporting,
        sgr_pixel_mouse: pane.sgr_pixel_mouse,
        pixel_width: pane.pixel_width,
        pixel_height: pane.pixel_height,
    }
}

fn project_split_hit(
    split: &crate::protocol::wire::PaneSurfaceSplit,
    area: Rect,
    topology_signature: u64,
) -> PaneSplitHit {
    PaneSplitHit {
        direction: split.direction,
        pos: match split.direction {
            crate::protocol::wire::PaneSurfaceSplitDirection::Horizontal => {
                area.x.saturating_add(split.pos)
            }
            crate::protocol::wire::PaneSurfaceSplitDirection::Vertical => {
                area.y.saturating_add(split.pos)
            }
        },
        area: Rect::new(
            area.x.saturating_add(split.area.x),
            area.y.saturating_add(split.area.y),
            split.area.width,
            split.area.height,
        ),
        hit_rect: Rect::new(
            area.x.saturating_add(split.hit_rect.x),
            area.y.saturating_add(split.hit_rect.y),
            split.hit_rect.width,
            split.hit_rect.height,
        ),
        path: split.path.clone(),
        topology_signature,
    }
}
