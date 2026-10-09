use super::pane_not_found;
use crate::protocol::api::schema::{
    EventData, EventEnvelope, EventKind, PaneLayoutPane, PaneLayoutParams, PaneLayoutRect,
    PaneLayoutSnapshot, PaneLayoutSplit, PaneSplitParams, PaneSwapParams, PaneSwapReason,
    PaneSwapResult, PaneZoomMode, PaneZoomParams, PaneZoomReason, PaneZoomResult, ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::app::App;
use crate::server::terminals::events::{PaneZoomCommand, PaneZoomNoopReason};
use crate::terminal::runtime::spawn::PaneShellConfig;
use crate::utils::ids::PaneId;

type PaneSwapTarget = (usize, usize, PaneId, Option<PaneId>, Option<PaneSwapReason>);

impl App {
    pub(in crate::server::api) fn handle_pane_split(
        &mut self,
        id: String,
        params: PaneSplitParams,
    ) -> String {
        let target = if let Some(target_pane_id) = params.target_pane_id.as_deref() {
            self.parse_pane_id(target_pane_id)
        } else if let Some(workspace_id) = params.workspace_id.as_deref() {
            self.parse_workspace_id(workspace_id).and_then(|ws_idx| {
                let pane_id = self.state.workspaces.get(ws_idx)?.focused_pane_id()?;
                Some((ws_idx, pane_id))
            })
        } else {
            self.state.active.and_then(|ws_idx| {
                let pane_id = self.state.workspaces.get(ws_idx)?.focused_pane_id()?;
                Some((ws_idx, pane_id))
            })
        };
        let Some((ws_idx, target_pane_id)) = target else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let extra_env = match crate::server::api::env::normalize_launch_env(params.env) {
            Ok(env) => env,
            Err((code, message)) => return encode_error(id, &code, message),
        };
        let (rows, cols) = self.state.estimate_pane_size();
        let split_cwd = params.cwd.map(std::path::PathBuf::from).or_else(|| {
            let follow_cwd = self.launch_cwd_for_pane_in_workspace(ws_idx, target_pane_id);
            Some(self.resolve_new_terminal_cwd(follow_cwd))
        });
        let default_shell = self.state.default_shell.clone();
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let host_terminal_appearance = self.state.host_terminal_appearance;
        let previous_focus = self.state.current_pane_focus_target();
        let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let direction = split_direction_to_layout(params.direction);
        let shell_config = PaneShellConfig::new(&default_shell, self.state.shell_mode);
        let split_result = match params.ratio {
            Some(ratio) => ws.split_pane_with_ratio(
                target_pane_id,
                direction,
                ratio,
                rows,
                cols,
                split_cwd,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                shell_config,
                extra_env,
                params.focus,
            ),
            None => ws.split_pane(
                target_pane_id,
                direction,
                rows,
                cols,
                split_cwd,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                shell_config,
                extra_env,
                params.focus,
            ),
        };
        let (target_tab_idx, new_pane) = match split_result {
            Some(Ok(result)) => result,
            Some(Err(err)) => return encode_error(id, "pane_split_failed", err.to_string()),
            None => return encode_error(id, "pane_not_found", "pane not found"),
        };
        if let Some(pane) = self.state.workspaces[ws_idx].pane_state_mut(new_pane.pane_id) {
            pane.right_click_passthrough = matches!(
                params.right_click,
                crate::protocol::api::schema::PaneRightClickTarget::Pane
            );
        }
        if params.focus {
            self.state.switch_workspace_tab(ws_idx, target_tab_idx);
            self.state
                .record_pane_focus_change(previous_focus, ws_idx, new_pane.pane_id);
            self.state.mode = crate::server::app::Mode::Terminal;
        }
        self.terminal_runtimes
            .insert(new_pane.terminal.id.clone(), new_pane.runtime);
        self.state
            .remove_alias_shadowed_by_new_pane(new_pane.pane_id);
        self.state
            .terminals
            .insert(new_pane.terminal.id.clone(), new_pane.terminal);
        self.schedule_session_save();
        let Some(pane) = self.pane_info(ws_idx, new_pane.pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        self.emit_event(EventEnvelope {
            event: EventKind::PaneCreated,
            data: EventData::PaneCreated { pane: pane.clone() },
        });
        self.emit_layout_updated_event(ws_idx, target_tab_idx);

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(in crate::server::api) fn handle_pane_layout(
        &mut self,
        id: String,
        params: PaneLayoutParams,
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
        let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) else {
            return encode_error(id, "pane_layout_unavailable", "pane layout unavailable");
        };

        encode_success(id, ResponseResult::PaneLayout { layout })
    }

    pub(in crate::server::api) fn handle_pane_swap(
        &mut self,
        id: String,
        params: PaneSwapParams,
    ) -> String {
        let directional = params.direction.is_some();
        let explicit = params.source_pane_id.is_some() || params.target_pane_id.is_some();
        if directional == explicit {
            return encode_error(
                id,
                "invalid_pane_swap",
                "provide either direction with optional pane_id, or source_pane_id and target_pane_id",
            );
        }

        let (ws_idx, tab_idx, source_pane_id, target_pane_id, reason) = if let Some(direction) =
            params.direction
        {
            let Some((ws_idx, source_pane_id)) =
                self.resolve_optional_pane(params.pane_id.as_deref())
            else {
                return encode_error(id, "pane_not_found", "source pane not found");
            };
            let Some(tab_idx) =
                self.state.workspaces[ws_idx].find_tab_index_for_pane(source_pane_id)
            else {
                return pane_not_found(
                    id,
                    &self
                        .public_pane_id(ws_idx, source_pane_id)
                        .unwrap_or_default(),
                );
            };
            let target = self.directional_pane_target(ws_idx, tab_idx, source_pane_id, direction);
            match target {
                Some(target_pane_id) => {
                    (ws_idx, tab_idx, source_pane_id, Some(target_pane_id), None)
                }
                None => (
                    ws_idx,
                    tab_idx,
                    source_pane_id,
                    None,
                    Some(PaneSwapReason::NoNeighbor),
                ),
            }
        } else {
            match self.resolve_explicit_pane_swap(&id, &params) {
                Ok(target) => target,
                Err(response) => return response,
            }
        };

        let mut changed = false;
        if reason.is_none() {
            if let Some(target_pane_id) = target_pane_id {
                let previous_focus = self.state.current_pane_focus_target();
                if let Some(tab) = self
                    .state
                    .workspaces
                    .get_mut(ws_idx)
                    .and_then(|ws| ws.tabs.get_mut(tab_idx))
                {
                    changed = tab.layout.swap_panes(source_pane_id, target_pane_id);
                    tab.layout.focus_pane(source_pane_id);
                    if changed {
                        self.state.switch_workspace_tab(ws_idx, tab_idx);
                        self.state
                            .record_pane_focus_change(previous_focus, ws_idx, source_pane_id);
                        self.state.mark_session_dirty();
                        self.schedule_session_save();
                    }
                }
            }
        }

        let (source_public_id, target_public_id) =
            self.pane_swap_public_ids(ws_idx, source_pane_id, target_pane_id, params);
        let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) else {
            return encode_error(id, "pane_layout_unavailable", "pane layout unavailable");
        };
        let focused_pane_id = layout.focused_pane_id.clone();
        if changed {
            self.emit_layout_updated_snapshot(layout.clone());
        }

        encode_success(
            id,
            ResponseResult::PaneSwap {
                swap: PaneSwapResult {
                    changed,
                    reason,
                    source_pane_id: source_public_id,
                    target_pane_id: target_public_id,
                    focused_pane_id,
                    layout,
                },
            },
        )
    }

    fn pane_swap_public_ids(
        &self,
        ws_idx: usize,
        source_pane_id: PaneId,
        target_pane_id: Option<PaneId>,
        params: PaneSwapParams,
    ) -> (String, Option<String>) {
        let source_public_id = match params.source_pane_id {
            Some(raw) => self
                .parse_pane_id(&raw)
                .and_then(|(idx, pane_id)| {
                    self.state
                        .workspaces
                        .get(idx)?
                        .find_tab_index_for_pane(pane_id)?;
                    self.public_pane_id(idx, pane_id)
                })
                .unwrap_or(raw),
            None => self
                .public_pane_id(ws_idx, source_pane_id)
                .unwrap_or_default(),
        };
        let target_public_id = match params.target_pane_id {
            Some(raw) => self
                .parse_pane_id(&raw)
                .and_then(|(idx, pane_id)| {
                    self.state
                        .workspaces
                        .get(idx)?
                        .find_tab_index_for_pane(pane_id)?;
                    self.public_pane_id(idx, pane_id)
                })
                .or(Some(raw)),
            None => target_pane_id.and_then(|pane_id| self.public_pane_id(ws_idx, pane_id)),
        };
        (source_public_id, target_public_id)
    }

    fn resolve_explicit_pane_swap(
        &self,
        id: &str,
        params: &PaneSwapParams,
    ) -> Result<PaneSwapTarget, String> {
        let Some(source_raw) = params.source_pane_id.as_deref() else {
            return Err(encode_error(
                id.to_owned(),
                "invalid_pane_swap",
                "missing source_pane_id",
            ));
        };
        let Some(target_raw) = params.target_pane_id.as_deref() else {
            return Err(encode_error(
                id.to_owned(),
                "invalid_pane_swap",
                "missing target_pane_id",
            ));
        };
        let source = self
            .parse_pane_id(source_raw)
            .and_then(|(ws_idx, pane_id)| {
                let tab_idx = self.state.workspaces[ws_idx].find_tab_index_for_pane(pane_id)?;
                Some((ws_idx, tab_idx, pane_id))
            });
        let target = self
            .parse_pane_id(target_raw)
            .and_then(|(ws_idx, pane_id)| {
                let tab_idx = self.state.workspaces[ws_idx].find_tab_index_for_pane(pane_id)?;
                Some((ws_idx, tab_idx, pane_id))
            });
        let response_context = source
            .map(|(ws_idx, tab_idx, _)| (ws_idx, tab_idx))
            .or_else(|| target.map(|(ws_idx, tab_idx, _)| (ws_idx, tab_idx)))
            .or_else(|| {
                let ws_idx = self.state.active?;
                let tab_idx = self.state.workspaces.get(ws_idx)?.active_tab_index();
                Some((ws_idx, tab_idx))
            });
        let Some((ws_idx, tab_idx)) = response_context else {
            return Err(encode_error(
                id.to_owned(),
                "pane_layout_unavailable",
                "pane layout unavailable",
            ));
        };
        let source_pane_id = source
            .map(|(_, _, pane_id)| pane_id)
            .or_else(|| {
                self.state
                    .workspaces
                    .get(ws_idx)?
                    .tabs
                    .get(tab_idx)
                    .map(|tab| tab.layout.focused())
            })
            .unwrap_or(PaneId::from_raw(0));
        let target_pane_id = target.map(|(_, _, pane_id)| pane_id);
        let reason = match (source, target) {
            (None, _) | (_, None) => Some(PaneSwapReason::NotFound),
            (Some((_, _, source)), Some((_, _, target))) if source == target => {
                Some(PaneSwapReason::SamePane)
            }
            (Some((source_ws, source_tab, _)), Some((target_ws, target_tab, _)))
                if source_ws != target_ws || source_tab != target_tab =>
            {
                Some(PaneSwapReason::CrossTab)
            }
            _ => None,
        };
        Ok((ws_idx, tab_idx, source_pane_id, target_pane_id, reason))
    }

    pub(in crate::server::api) fn handle_pane_zoom(
        &mut self,
        id: String,
        params: PaneZoomParams,
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
        let command = match params.mode {
            PaneZoomMode::Toggle => PaneZoomCommand::Toggle,
            PaneZoomMode::On => PaneZoomCommand::On,
            PaneZoomMode::Off => PaneZoomCommand::Off,
        };
        let Some(outcome) = self.state.apply_pane_zoom(ws_idx, pane_id, command) else {
            return pane_not_found(id, &pane_public_id);
        };
        if outcome.changed || outcome.focus_changed {
            self.schedule_session_save();
        }
        self.state.mode = crate::server::app::Mode::Terminal;
        let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) else {
            return encode_error(id, "pane_layout_unavailable", "pane layout unavailable");
        };
        let focused_pane_id = layout.focused_pane_id.clone();
        if outcome.changed || outcome.focus_changed {
            self.emit_layout_updated_snapshot(layout.clone());
        }

        encode_success(
            id,
            ResponseResult::PaneZoom {
                zoom: PaneZoomResult {
                    changed: outcome.changed || outcome.focus_changed,
                    zoom_changed: outcome.changed,
                    focus_changed: outcome.focus_changed,
                    reason: outcome.reason.map(|reason| match reason {
                        PaneZoomNoopReason::SinglePane => PaneZoomReason::SinglePane,
                        PaneZoomNoopReason::AlreadyZoomed => PaneZoomReason::AlreadyZoomed,
                        PaneZoomNoopReason::AlreadyUnzoomed => PaneZoomReason::AlreadyUnzoomed,
                    }),
                    pane_id: pane_public_id,
                    focused_pane_id,
                    zoomed: outcome.zoomed,
                    layout,
                },
            },
        )
    }

    pub(in crate::server::api) fn pane_layout_snapshot(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<PaneLayoutSnapshot> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.get(tab_idx)?;
        let area = self.state.view.terminal_area;
        let focused_pane_id = self.public_pane_id(ws_idx, tab.layout.focused())?;
        let panes = crate::server::rendering::surface::apply_pane_chrome(
            tab.layout.panes(area),
            self.state.pane_borders,
            self.state.pane_gaps,
            self.state.pane_outer_borders,
        )
        .into_iter()
        .filter_map(|pane| {
            Some(PaneLayoutPane {
                pane_id: self.public_pane_id(ws_idx, pane.id)?,
                focused: pane.is_focused,
                rect: pane.rect.into(),
            })
        })
        .collect();
        let splits = tab
            .layout
            .splits(area)
            .into_iter()
            .enumerate()
            .map(|(idx, split)| PaneLayoutSplit {
                id: split_path_id(idx, &split.path),
                direction: match split.direction {
                    ratatui::layout::Direction::Horizontal => {
                        crate::protocol::api::schema::SplitDirection::Right
                    }
                    ratatui::layout::Direction::Vertical => {
                        crate::protocol::api::schema::SplitDirection::Down
                    }
                },
                ratio: split.ratio,
                rect: split.area.into(),
            })
            .collect();

        Some(PaneLayoutSnapshot {
            workspace_id: self.public_workspace_id(ws_idx),
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            zoomed: tab.zoomed,
            area: area.into(),
            focused_pane_id,
            panes,
            splits,
        })
    }

    pub(crate) fn emit_layout_updated_event(&mut self, ws_idx: usize, tab_idx: usize) {
        if let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) {
            self.emit_layout_updated_snapshot(layout);
        }
    }

    pub(in crate::server::api) fn emit_layout_updated_snapshot(
        &mut self,
        layout: PaneLayoutSnapshot,
    ) {
        self.emit_event(EventEnvelope {
            event: EventKind::LayoutUpdated,
            data: EventData::LayoutUpdated { layout },
        });
    }

    pub(crate) fn layout_update_target_after_pane_removal(
        &self,
        ws_idx: usize,
        pane_id: PaneId,
    ) -> Option<(usize, usize)> {
        let tab_idx = self
            .state
            .workspaces
            .get(ws_idx)?
            .find_tab_index_for_pane(pane_id)?;
        let pane_count = self
            .state
            .workspaces
            .get(ws_idx)?
            .tabs
            .get(tab_idx)?
            .layout
            .pane_count();
        (pane_count > 1).then_some((ws_idx, tab_idx))
    }
}

fn split_direction_to_layout(
    direction: crate::protocol::api::schema::SplitDirection,
) -> ratatui::layout::Direction {
    match direction {
        crate::protocol::api::schema::SplitDirection::Right => {
            ratatui::layout::Direction::Horizontal
        }
        crate::protocol::api::schema::SplitDirection::Down => ratatui::layout::Direction::Vertical,
    }
}

pub(super) fn split_path_id(idx: usize, path: &[bool]) -> String {
    if path.is_empty() {
        return format!("split_{idx}_root");
    }
    let path = path
        .iter()
        .map(|right| if *right { "1" } else { "0" })
        .collect::<Vec<_>>()
        .join("");
    format!("split_{idx}_{path}")
}

impl From<ratatui::layout::Rect> for PaneLayoutRect {
    fn from(rect: ratatui::layout::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}
