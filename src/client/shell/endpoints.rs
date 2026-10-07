use super::*;

impl ClientShellState {
    pub(crate) fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Installs a server snapshot, dropping one older than the snapshot already shown.
    pub(crate) fn install_snapshot(&mut self, mut snapshot: Box<ClientShellSnapshot>) {
        if self.snapshot.as_deref().is_some_and(|previous| {
            previous.boot_id == snapshot.boot_id && previous.revision > snapshot.revision
        }) {
            return;
        }
        self.agent_presentation.project_snapshot(&mut snapshot);
        if let Some(surface) = self.pane_surface.as_ref() {
            self.agent_presentation
                .acknowledge_surface(&mut snapshot, surface, self.outer_focused);
        }
        self.apply_active_snapshot(snapshot);
    }

    pub(crate) fn acknowledge_active_surface_agents(&mut self, surface: &PaneSurfaceFrame) -> bool {
        let Some(snapshot) = self.snapshot.as_deref_mut() else {
            return false;
        };
        self.agent_presentation
            .acknowledge_surface(snapshot, surface, self.outer_focused)
    }

    #[cfg(test)]
    pub(crate) fn set_snapshot(&mut self, snapshot: Box<ClientShellSnapshot>) {
        self.install_snapshot(snapshot);
    }
}
