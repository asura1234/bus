//! Virtual rendering helpers for headless client frame streaming.

use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::state::AppState;
#[cfg(test)]
use crate::protocol::FrameData;
use crate::protocol::{CursorState, PaneSurfaceFrame, PaneSurfacePatch, ServerMessage};
use crate::terminal::TerminalRuntimeRegistry;

/// Per-client delivery baseline for semantic pane surfaces.
#[derive(Default)]
pub(crate) struct ClientRenderState {
    last_surface: Option<Box<PaneSurfaceFrame>>,
    surface_revision: u64,
}

impl ClientRenderState {
    pub(crate) fn reset_baseline(&mut self) {
        self.last_surface = None;
    }

    pub(crate) fn request_repaint(&mut self) {
        self.reset_baseline();
    }

    pub(crate) fn last_pane_surface(&self) -> Option<&PaneSurfaceFrame> {
        self.last_surface.as_deref()
    }

    pub(crate) fn prepare_pane_surface(
        &mut self,
        mut surface: PaneSurfaceFrame,
    ) -> Option<PreparedRender> {
        if surface.graphics.assets.is_empty()
            && self.last_surface.as_deref().is_some_and(|last| {
                last.projection_revision == surface.projection_revision
                    && last.frame == surface.frame
                    && last.panes == surface.panes
                    && last.splits == surface.splits
                    && last.graphics.placements == surface.graphics.placements
            })
        {
            return None;
        }
        surface.surface_revision = self.surface_revision.saturating_add(1);
        let mut committed_surface = surface.clone();
        committed_surface.graphics.assets.clear();
        Some(PreparedRender {
            message: ServerMessage::PaneSurface(surface),
            committed_surface: Box::new(committed_surface),
        })
    }

    pub(crate) fn prepare_pane_surface_patch(
        &mut self,
        mut patch: PaneSurfacePatch,
        mut committed_surface: PaneSurfaceFrame,
    ) -> Option<PreparedRender> {
        let last = self.last_surface.as_deref()?;
        if last.boot_id != patch.boot_id
            || last.projection_revision != patch.projection_revision
            || last.surface_revision != patch.base_surface_revision
        {
            return None;
        }
        let next_revision = self.surface_revision.saturating_add(1);
        patch.surface_revision = next_revision;
        committed_surface.surface_revision = next_revision;
        committed_surface.graphics.assets.clear();
        Some(PreparedRender {
            message: ServerMessage::PaneSurfacePatch(patch),
            committed_surface: Box::new(committed_surface),
        })
    }

    pub(crate) fn commit_sent_frame(&mut self, prepared: PreparedRender) {
        self.surface_revision = prepared.committed_surface.surface_revision;
        self.last_surface = Some(prepared.committed_surface);
    }
}

/// A prepared client render message and its baseline after a successful send.
pub(crate) struct PreparedRender {
    message: ServerMessage,
    committed_surface: Box<PaneSurfaceFrame>,
}

impl PreparedRender {
    pub(crate) fn message(&self) -> &ServerMessage {
        &self.message
    }

    pub(crate) fn strip_pane_surface_assets(&mut self) -> bool {
        let ServerMessage::PaneSurface(surface) = &mut self.message else {
            return false;
        };
        if surface.graphics.assets.is_empty() {
            return false;
        }
        surface.graphics.assets.clear();
        true
    }
}

pub(crate) type RenderedTabSurface = (
    ratatui::buffer::Buffer,
    Option<CursorState>,
    Vec<((u16, u16), String, String)>,
    crate::ui::TabSurfaceLayout,
);

/// Renders only the active tab's pane surface at an origin-relative client viewport.
pub(crate) fn render_tab_surface_virtual(
    app_state: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    target: Option<crate::ui::TabSurfaceTarget>,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> RenderedTabSurface {
    let layout = crate::ui::compute_tab_surface_for(
        app_state,
        terminal_runtimes,
        target,
        area,
        resize_panes,
        cell_size,
    );
    let surface = crate::ui::TabSurfaceView {
        target: layout.target,
        pane_infos: &layout.pane_infos,
        split_borders: &layout.split_borders,
    };
    let cursor = crate::ui::tab_surface_cursor(app_state, terminal_runtimes, surface);
    let hyperlinks = crate::ui::tab_surface_hyperlinks(app_state, terminal_runtimes, surface);

    let backend = TestBackend::new(area.width, area.height);
    let mut terminal = ratatui::Terminal::new(backend).expect("TestBackend::new should never fail");
    terminal
        .draw(|frame| {
            crate::ui::render_tab_surface(app_state, terminal_runtimes, surface, frame);
        })
        .expect("render to TestBackend should never fail");

    (
        terminal.backend().buffer().clone(),
        cursor,
        hyperlinks,
        layout,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(content: &str) -> PaneSurfaceFrame {
        let pane = ratatui::buffer::Buffer::with_lines([content]);
        PaneSurfaceFrame {
            boot_id: "boot-1".into(),
            projection_revision: 1,
            surface_revision: 1,
            frame: FrameData::from_ratatui_buffer_with_hyperlinks(&pane, None, &[]),
            panes: Vec::new(),
            splits: Vec::new(),
            graphics: crate::protocol::SurfaceGraphicsScene::default(),
        }
    }

    #[test]
    fn forced_full_surface_keeps_the_connection_revision_monotonic() {
        let mut state = ClientRenderState::default();
        let prepared = state
            .prepare_pane_surface(surface("first"))
            .expect("initial surface");
        state.commit_sent_frame(prepared);
        state.request_repaint();

        let prepared = state
            .prepare_pane_surface(surface("replacement"))
            .expect("forced replacement surface");
        assert!(matches!(
            prepared.message(),
            ServerMessage::PaneSurface(surface) if surface.surface_revision == 2
        ));
        state.commit_sent_frame(prepared);
        assert_eq!(state.last_pane_surface().unwrap().surface_revision, 2);
    }
}
