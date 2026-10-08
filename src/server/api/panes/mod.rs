mod copy;
mod io;
mod layout;
mod navigation;
mod session;

use crate::protocol::api::schema::{
    EventData, EventEnvelope, EventKind, PaneCurrentParams, PaneListParams, PaneRenameParams,
    PaneTarget, ResponseResult,
};
#[cfg(test)]
use crate::protocol::api::schema::{
    PaneCopyMotion, PaneCopyMotionParams, PaneCopySearchDirection, PaneCopySearchParams,
    PaneDirection, PaneFocusDirectionReason, PaneInputSetParams, PaneReadParams,
    PaneReportAgentSessionParams, PaneScrollParams, PaneSelectionReadParams, PaneSendInputParams,
    PaneSendKeysParams, PaneSwapParams, PaneSwapReason, PaneTextPoint, PaneZoomMode,
    PaneZoomParams, PaneZoomReason,
};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::app::App;
#[cfg(test)]
use crate::server::app::Mode;
use crate::server::workspaces::layout::PaneId;

impl App {
    pub(super) fn handle_pane_list(&mut self, id: String, params: PaneListParams) -> String {
        match self.collect_panes_for_workspace(params.workspace_id.as_deref()) {
            Ok(panes) => encode_success(id, ResponseResult::PaneList { panes }),
            Err((code, message)) => encode_error(id, &code, message),
        }
    }

    pub(super) fn handle_pane_current(&mut self, id: String, params: PaneCurrentParams) -> String {
        let target = match params.caller_pane_id.as_deref() {
            Some(caller_pane_id) => self.parse_pane_id(caller_pane_id),
            None => self.resolve_optional_pane(None),
        };
        let Some((ws_idx, pane_id)) = target else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };

        encode_success(id, ResponseResult::PaneCurrent { pane })
    }

    pub(super) fn handle_pane_get(&mut self, id: String, target: PaneTarget) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&target.pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_focus(&mut self, id: String, target: PaneTarget) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&target.pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };
        let Some(_tab_idx) = self.state.workspaces[ws_idx].find_tab_index_for_pane(pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };

        self.state.focus_pane_in_workspace(ws_idx, pane_id);
        self.state.mark_active_tab_seen();
        self.state.mode = crate::server::app::Mode::Terminal;

        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };
        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_rename(&mut self, id: String, params: PaneRenameParams) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.terminal_id(pane_id))
            .cloned()
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        match params.label.map(|label| label.trim().to_string()) {
            Some(label) if !label.is_empty() => terminal.set_manual_label(label),
            _ => terminal.clear_manual_label(),
        }
        self.state.mark_session_dirty();
        let pane = self.pane_info(ws_idx, pane_id).unwrap();

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_close(&mut self, id: String, target: PaneTarget) -> String {
        match self.close_pane(id.clone(), &target) {
            Ok(()) => encode_success(id, ResponseResult::Ok {}),
            Err(response) => response,
        }
    }

    pub(super) fn handle_pane_close_if_identity(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::PaneCloseIfIdentityParams,
    ) -> String {
        if params.expected_terminal_id.is_empty()
            || params.expected_agent.is_empty()
            || params.expected_managed_name.is_empty()
        {
            return encode_error(
                id,
                "invalid_request",
                "Exact terminal ownership is required",
            );
        }
        // A lost response is safe to retry, including when the old pane id now
        // refers to a different terminal. Never close that replacement pane.
        let terminal_id = self
            .state
            .terminals
            .keys()
            .find(|terminal| terminal.to_string() == params.expected_terminal_id)
            .cloned();
        let Some(terminal_id) = terminal_id else {
            if self
                .terminal_runtimes
                .contains_id(&params.expected_terminal_id)
            {
                return encode_error(
                    id,
                    "terminal_identity_changed",
                    "Terminal runtime has no verifiable owner",
                );
            }
            return encode_success(id, ResponseResult::Ok {});
        };
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(
                id,
                "terminal_identity_changed",
                "Owned terminal moved; pane was not closed",
            );
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return encode_error(
                id,
                "terminal_identity_changed",
                "Pane identity is unavailable",
            );
        };
        let terminal = &self.state.terminals[&terminal_id];
        if pane.terminal_id != params.expected_terminal_id
            || pane.pane_id != params.pane_id
            || pane.agent.as_deref() != Some(params.expected_agent.as_str())
            || terminal.agent_name.as_deref() != Some(params.expected_managed_name.as_str())
            || pane
                .agent_session
                .as_ref()
                .map(|session| session.value.as_str())
                != params.expected_session_id.as_deref()
        {
            return encode_error(
                id,
                "terminal_identity_changed",
                "Terminal or provider session changed; pane was not closed",
            );
        }
        let attachment_count = self
            .state
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.tabs)
            .flat_map(|tab| tab.panes.values())
            .filter(|pane| pane.attached_terminal_id == terminal_id)
            .count();
        if attachment_count != 1 {
            return encode_error(
                id,
                "terminal_identity_changed",
                "Terminal is shared; pane was not closed",
            );
        }
        // Validate and stop in the same server dispatch, without an asynchronous
        // gap in which the pane can be rebound. Keep state on shutdown failure.
        if let Some(runtime) = self.terminal_runtimes.get(&terminal_id) {
            if !runtime.stop_session_for_close() {
                return encode_error(
                    id,
                    "terminal_stop_failed",
                    "Terminal session is still running; retry deletion",
                );
            }
        }
        self.handle_pane_close(
            id,
            PaneTarget {
                pane_id: params.pane_id,
            },
        )
    }

    /// Close a pane; `Err` carries the encoded error response.
    pub(super) fn close_pane(&mut self, id: String, target: &PaneTarget) -> Result<(), String> {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&target.pane_id) else {
            return Err(pane_not_found(id, &target.pane_id));
        };
        let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
            return Err(pane_not_found(id, &target.pane_id));
        };
        let workspace_id = self.public_workspace_id(ws_idx);
        let layout_update_target = self.layout_update_target_after_pane_removal(ws_idx, pane_id);
        let workspace_snapshot = self.workspace_info(ws_idx);
        let terminal_id = self.state.terminal_id_for_pane(ws_idx, pane_id);
        let should_close_workspace = {
            let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
                return Err(pane_not_found(id, &target.pane_id));
            };
            ws.close_pane(pane_id)
        };
        self.state.forget_closed_pane_focus([pane_id]);
        if should_close_workspace {
            self.state.selected = ws_idx;
            self.state.close_selected_workspace();
            self.shutdown_detached_terminal_runtimes();
            self.emit_event(EventEnvelope {
                event: EventKind::PaneClosed,
                data: EventData::PaneClosed {
                    pane_id: public_pane_id,
                    workspace_id: workspace_id.clone(),
                },
            });
            self.emit_event(EventEnvelope {
                event: EventKind::WorkspaceClosed,
                data: EventData::WorkspaceClosed {
                    workspace_id,
                    workspace: Some(workspace_snapshot),
                },
            });
        } else {
            self.state.remove_unattached_terminal_ids(terminal_id);
            self.shutdown_detached_terminal_runtimes();
            self.schedule_session_save();
            self.emit_event(EventEnvelope {
                event: EventKind::PaneClosed,
                data: EventData::PaneClosed {
                    pane_id: public_pane_id,
                    workspace_id,
                },
            });
            if let Some((ws_idx, tab_idx)) = layout_update_target {
                self.emit_layout_updated_event(ws_idx, tab_idx);
            }
        }

        Ok(())
    }

    fn resolve_optional_pane(&self, pane_id: Option<&str>) -> Option<(usize, PaneId)> {
        match pane_id {
            Some(pane_id) => self.parse_pane_id(pane_id),
            None => {
                let ws_idx = self.state.active?;
                let pane_id = self.state.workspaces.get(ws_idx)?.focused_pane_id()?;
                Some((ws_idx, pane_id))
            }
        }
    }
}

fn pane_not_found(id: String, pane_id: &str) -> String {
    encode_error(id, "pane_not_found", format!("pane {pane_id} not found"))
}

#[cfg(test)]
#[path = "tests/session_test.rs"]
mod tests;
