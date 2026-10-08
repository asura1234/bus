use super::pane_not_found;
use crate::protocol::api::schema::{
    PaneDirection, PaneFocusDirectionParams, PaneFocusDirectionReason, PaneFocusDirectionResult,
    PaneResizeParams, PaneResizeReason, PaneResizeResult, ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::app::App;
use crate::server::workspaces::layout::{find_in_direction, NavDirection, PaneId};

impl App {
    pub(in crate::server::api) fn handle_pane_focus_direction(
        &mut self,
        id: String,
        params: PaneFocusDirectionParams,
    ) -> String {
        let Some((ws_idx, source_pane_id)) = self.resolve_optional_pane(params.pane_id.as_deref())
        else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let Some(tab_idx) = self.state.workspaces[ws_idx].find_tab_index_for_pane(source_pane_id)
        else {
            return pane_not_found(
                id,
                &self
                    .public_pane_id(ws_idx, source_pane_id)
                    .unwrap_or_default(),
            );
        };
        let Some(source_public_id) = self.public_pane_id(ws_idx, source_pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let target =
            self.directional_pane_target(ws_idx, tab_idx, source_pane_id, params.direction);
        let reason = target
            .is_none()
            .then_some(PaneFocusDirectionReason::NoNeighbor);

        if let Some(target_pane_id) = target {
            self.state.focus_pane_in_workspace(ws_idx, target_pane_id);
            self.state.switch_workspace_tab(ws_idx, tab_idx);
            self.state.mode = crate::server::app::Mode::Terminal;
        }
        let focused_pane_id = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(tab_idx))
            .map(|tab| tab.layout.focused())
            .and_then(|pane_id| self.public_pane_id(ws_idx, pane_id));
        let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) else {
            return encode_error(id, "pane_layout_unavailable", "pane layout unavailable");
        };

        encode_success(
            id,
            ResponseResult::PaneFocusDirection {
                focus: PaneFocusDirectionResult {
                    changed: target.is_some(),
                    reason,
                    source_pane_id: source_public_id,
                    focused_pane_id,
                    layout,
                },
            },
        )
    }

    pub(in crate::server::api) fn handle_pane_resize(
        &mut self,
        id: String,
        params: PaneResizeParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.resolve_optional_pane(params.pane_id.as_deref()) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let Some(tab_idx) = self.state.workspaces[ws_idx].find_tab_index_for_pane(pane_id) else {
            return pane_not_found(
                id,
                &self.public_pane_id(ws_idx, pane_id).unwrap_or_default(),
            );
        };
        let Some(pane_public_id) = self.public_pane_id(ws_idx, pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };

        let amount = params
            .amount
            .filter(|amount| amount.is_finite())
            .unwrap_or(0.05)
            .abs()
            .min(0.5);
        let direction: NavDirection = params.direction.into();
        let area = self.state.view.terminal_area;
        let changed = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .and_then(|ws| ws.tabs.get_mut(tab_idx))
            .is_some_and(|tab| tab.layout.resize_pane(pane_id, direction, amount, area));
        if changed {
            self.schedule_session_save();
        }

        let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) else {
            return encode_error(id, "pane_layout_unavailable", "pane layout unavailable");
        };
        let focused_pane_id = layout.focused_pane_id.clone();
        if changed {
            self.emit_layout_updated_snapshot(layout.clone());
        }

        encode_success(
            id,
            ResponseResult::PaneResize {
                resize: PaneResizeResult {
                    changed,
                    reason: (!changed).then_some(PaneResizeReason::Unchanged),
                    pane_id: pane_public_id,
                    focused_pane_id,
                    layout,
                },
            },
        )
    }

    pub(super) fn directional_pane_target(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        source_pane_id: PaneId,
        direction: PaneDirection,
    ) -> Option<PaneId> {
        let tab = self.state.workspaces.get(ws_idx)?.tabs.get(tab_idx)?;
        let panes = tab.layout.panes(self.state.view.terminal_area);
        let source = panes.iter().find(|pane| pane.id == source_pane_id)?;
        find_in_direction(source, direction.into(), &panes)
    }
}

impl From<PaneDirection> for NavDirection {
    fn from(direction: PaneDirection) -> Self {
        match direction {
            PaneDirection::Left => NavDirection::Left,
            PaneDirection::Right => NavDirection::Right,
            PaneDirection::Up => NavDirection::Up,
            PaneDirection::Down => NavDirection::Down,
        }
    }
}
