//! Retained terminal text for immediate agent navigation while native focus catches up.
use super::{blit_pane_surface, Buffer, ClientShellState, FrameData, ShellHitMap};
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, Widget};

impl ClientShellState {
    pub(super) fn retain_bus_terminal_surface(&mut self) {
        if self.bus.is_none() {
            return;
        }
        let Some((snapshot, surface)) = self.snapshot.as_deref().zip(self.pane_surface.as_ref())
        else {
            return;
        };
        if surface.boot_id != snapshot.boot_id {
            return;
        }
        let Some(tab) = surface.panes.first().and_then(|pane| {
            snapshot
                .panes
                .iter()
                .find(|known| known.pane_id == pane.pane_id)
                .map(|known| known.tab_id.clone())
        }) else {
            return;
        };
        self.bus_terminal_surfaces.insert(tab, surface.clone());
    }

    pub(super) fn prune_bus_terminal_surfaces(&mut self) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        self.bus_terminal_surfaces.retain(|tab, surface| {
            surface.boot_id == snapshot.boot_id
                && surface.panes.iter().all(|pane| {
                    snapshot
                        .panes
                        .iter()
                        .any(|known| known.pane_id == pane.pane_id && &known.tab_id == tab)
                })
        });
    }

    pub(super) fn compose_bus_terminal_preview(
        &mut self,
        cols: u16,
        rows: u16,
    ) -> Option<FrameData> {
        let bus = self.bus.as_ref()?;
        let target = bus.terminal_pane()?;
        let snapshot = self.snapshot.as_deref()?;
        // A removed pane or a different server boot must never borrow an old terminal image.
        snapshot.panes.iter().find(|pane| pane.pane_id == target)?;
        let matches = |surface: &&crate::protocol::wire::PaneSurfaceFrame| {
            surface.boot_id == snapshot.boot_id
                && surface.projection_revision <= snapshot.revision
                && surface.panes.iter().any(|pane| pane.pane_id == target)
        };
        let surface = self
            .pane_surface
            .as_ref()
            .filter(matches)
            .or_else(|| self.bus_terminal_surfaces.values().find(matches))?;
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, rows));
        bus.render(&mut buffer);
        Clear.render(self.layout(cols, rows).pane_surface, &mut buffer);
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        blit_pane_surface(
            &mut frame,
            &surface.frame,
            self.layout(cols, rows).pane_surface,
        );
        // Preview geometry is stale: no pane mouse hits, graphics, selection or input leases.
        self.hits = ShellHitMap::default();
        self.presented_bus_terminal = Some(target.to_owned());
        Some(frame)
    }
}
