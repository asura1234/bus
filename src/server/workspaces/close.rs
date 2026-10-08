use crate::layout::PaneId;
use crate::server::app_state::{AppState, Mode};
use tracing::warn;

impl AppState {
    pub(crate) fn terminal_ids_for_workspace(
        &self,
        ws_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.workspaces
            .get(ws_idx)
            .into_iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn pane_ids_for_workspace(&self, ws_idx: usize) -> Vec<PaneId> {
        self.workspaces
            .get(ws_idx)
            .into_iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| tab.layout.pane_ids())
            .collect()
    }

    pub(crate) fn terminal_ids_for_tab(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(tab_idx))
            .into_iter()
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn terminal_id_for_pane(
        &self,
        ws_idx: usize,
        pane_id: PaneId,
    ) -> Option<crate::terminal::TerminalId> {
        self.workspaces
            .get(ws_idx)?
            .pane_state(pane_id)
            .map(|pane| pane.attached_terminal_id.clone())
    }

    pub(crate) fn remove_unattached_terminal_ids(
        &mut self,
        terminal_ids: impl IntoIterator<Item = crate::terminal::TerminalId>,
    ) {
        for terminal_id in terminal_ids {
            let still_attached = self.workspaces.iter().any(|ws| {
                ws.tabs.iter().any(|tab| {
                    tab.panes
                        .values()
                        .any(|pane| pane.attached_terminal_id == terminal_id)
                })
            });
            if !still_attached
                && self.terminals.remove(&terminal_id).is_some()
                && !self.terminal_runtime_shutdowns.contains(&terminal_id)
            {
                self.terminal_runtime_shutdowns.push(terminal_id);
            }
        }
    }

    pub(crate) fn forget_closed_pane_focus(&mut self, pane_ids: impl IntoIterator<Item = PaneId>) {
        let pane_ids = pane_ids.into_iter().collect::<Vec<_>>();
        if self
            .previous_pane_focus
            .as_ref()
            .is_some_and(|focus| pane_ids.contains(&focus.pane_id))
        {
            self.previous_pane_focus = None;
        }
    }

    pub fn close_selected_workspace(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        self.mark_session_dirty();
        let close_indices = [self.selected];

        let mut terminal_ids = Vec::new();
        let mut pane_ids = Vec::new();
        for idx in &close_indices {
            terminal_ids.extend(self.terminal_ids_for_workspace(*idx));
            pane_ids.extend(self.pane_ids_for_workspace(*idx));
            if let Some(workspace_id) = self.workspaces.get(*idx).map(|ws| ws.id.clone()) {
                crate::logging::workspace_closed(&workspace_id);
            }
        }
        let active_workspace_id = self
            .active
            .and_then(|idx| self.workspaces.get(idx))
            .map(|ws| ws.id.clone());
        self.forget_closed_pane_focus(pane_ids);
        for idx in close_indices.iter().rev() {
            self.workspaces.remove(*idx);
        }
        self.remove_unattached_terminal_ids(terminal_ids);
        if self.workspaces.is_empty() {
            self.active = None;
            self.selected = 0;
        } else {
            // Keep focus on the previously focused workspace
            if let Some(id) = active_workspace_id {
                if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == id) {
                    self.selected = idx;
                }
            }
            if self.selected >= self.workspaces.len() {
                self.selected = self.workspaces.len() - 1;
            }
            self.active = Some(self.selected);
        }
    }

    #[cfg(test)]
    /// Close the focused pane. Returns true when the close was deferred to confirmation.
    pub fn close_pane(&mut self) -> bool {
        let active = self.active;
        self.mark_session_dirty();
        let terminal_ids = active
            .and_then(|i| {
                self.workspaces
                    .get(i)
                    .and_then(|ws| ws.focused_pane_id().map(|pane_id| (i, pane_id)))
            })
            .and_then(|(i, pane_id)| self.terminal_id_for_pane(i, pane_id))
            .into_iter()
            .collect::<Vec<_>>();
        let pane_ids = active
            .and_then(|i| self.workspaces.get(i).and_then(|ws| ws.focused_pane_id()))
            .into_iter()
            .collect::<Vec<_>>();
        let should_close_workspace = active
            .and_then(|i| self.workspaces.get_mut(i))
            .is_some_and(|ws| ws.close_focused());
        self.forget_closed_pane_focus(pane_ids);
        if should_close_workspace {
            if let Some(active) = active {
                self.selected = active;
            }
            self.close_selected_workspace();
        } else {
            self.remove_unattached_terminal_ids(terminal_ids);
        }
        false
    }

    pub(in crate::server) fn handle_pane_died(&mut self, pane_id: PaneId) {
        self.pending_agent_notifications.remove(&pane_id);
        self.forget_closed_pane_focus([pane_id]);
        let ws_idx = self
            .workspaces
            .iter()
            .position(|ws| ws.find_tab_index_for_pane(pane_id).is_some());

        let Some(ws_idx) = ws_idx else {
            warn!(target: "bus::server::terminals::events", pane = pane_id.raw(), "PaneDied for unknown pane");
            return;
        };

        let pane_terminal_id = self.terminal_id_for_pane(ws_idx, pane_id);
        let workspace_terminal_ids = self.terminal_ids_for_workspace(ws_idx);
        self.pane_id_aliases.retain(|_, alias| *alias != pane_id);
        self.public_pane_id_aliases
            .retain(|_, alias| *alias != pane_id);
        let should_close_workspace = {
            let ws = &mut self.workspaces[ws_idx];
            ws.remove_pane(pane_id)
        };
        self.mark_session_dirty();

        if should_close_workspace {
            let active_workspace_id = self
                .active
                .and_then(|idx| self.workspaces.get(idx))
                .map(|ws| ws.id.clone());
            let selected_workspace_id = self.workspaces.get(self.selected).map(|ws| ws.id.clone());
            self.workspaces.remove(ws_idx);
            self.remove_unattached_terminal_ids(workspace_terminal_ids);
            if self.workspaces.is_empty() {
                self.active = None;
                self.selected = 0;
                if self.mode == Mode::Terminal {
                    self.mode = Mode::Navigate;
                }
            } else {
                // Keep focus on the previously focused workspace
                if let Some(id) = active_workspace_id {
                    if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == id) {
                        self.active = Some(idx);
                    }
                }
                if let Some(active) = self.active {
                    if active >= self.workspaces.len() {
                        self.active = Some(self.workspaces.len() - 1);
                    }
                }
                if let Some(id) = selected_workspace_id {
                    if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == id) {
                        self.selected = idx;
                    }
                }
                if self.selected >= self.workspaces.len() {
                    self.selected = self.workspaces.len() - 1;
                }
            }
        } else {
            self.remove_unattached_terminal_ids(pane_terminal_id);
        }
    }
}
