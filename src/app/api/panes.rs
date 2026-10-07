use bytes::Bytes;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, PaneCopyMotion, PaneCopyMotionParams,
    PaneCopySearchDirection, PaneCopySearchParams, PaneCurrentParams, PaneDirection,
    PaneFocusDirectionParams, PaneFocusDirectionReason, PaneFocusDirectionResult,
    PaneInputSetParams, PaneLayoutPane, PaneLayoutParams, PaneLayoutRect, PaneLayoutSnapshot,
    PaneLayoutSplit, PaneLinkActivateParams, PaneListParams, PaneReadParams, PaneReadResult,
    PaneRenameParams, PaneReportAgentSessionParams, PaneResizeParams, PaneResizeReason,
    PaneResizeResult, PaneScrollParams, PaneSelectionReadParams, PaneSendInputParams,
    PaneSendKeysParams, PaneSendTextParams, PaneSplitParams, PaneSwapParams, PaneSwapReason,
    PaneSwapResult, PaneTarget, PaneTextPoint, PaneTextRange, PaneZoomMode, PaneZoomParams,
    PaneZoomReason, PaneZoomResult, ResponseResult,
};
use crate::app::actions::{PaneZoomCommand, PaneZoomNoopReason};
use crate::app::App;
#[cfg(test)]
use crate::app::Mode;
use crate::layout::{find_in_direction, NavDirection, PaneId};

use super::super::api_helpers::{encode_api_keys, normalize_reported_agent_label};
use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_pane_link_activate(
        &mut self,
        id: String,
        params: PaneLinkActivateParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        if !self.state.pane_visible_on_active_surface(ws_idx, pane_id) {
            return encode_error(id, "stale_target", "pane is no longer visible");
        }
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return encode_error(id, "pane_not_found", "pane runtime not found");
        };
        let current_offset = runtime
            .scroll_metrics()
            .map(|metrics| metrics.offset_from_bottom as u64);
        if params
            .offset_from_bottom
            .is_some_and(|expected| current_offset != Some(expected))
        {
            return encode_error(
                id,
                "stale_content",
                "pane viewport changed before link activation",
            );
        }
        let content_revision = runtime.content_seq();
        if content_revision % 2 != 0
            || params
                .content_revision
                .is_some_and(|expected| expected != content_revision)
        {
            return encode_error(
                id,
                "stale_content",
                "pane content changed before link activation",
            );
        }
        let url = self.state.url_at_pane_surface_cell(
            &self.terminal_runtimes,
            ws_idx,
            pane_id,
            params.viewport_row,
            params.col,
        );
        if runtime.content_seq() != content_revision
            || runtime
                .scroll_metrics()
                .map(|metrics| metrics.offset_from_bottom as u64)
                != current_offset
        {
            return encode_error(
                id,
                "stale_content",
                "pane content or viewport changed during link activation",
            );
        }
        // The client opens the resolved URL itself.
        encode_success(
            id,
            ResponseResult::PaneLinkActivated {
                url,
                handled: false,
            },
        )
    }

    pub(super) fn handle_pane_split(&mut self, id: String, params: PaneSplitParams) -> String {
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
        let extra_env = match super::env::normalize_launch_env(params.env) {
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
        let shell_config = crate::pane::PaneShellConfig::new(&default_shell, self.state.shell_mode);
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
                crate::api::schema::PaneRightClickTarget::Pane
            );
        }
        if params.focus {
            self.state.switch_workspace_tab(ws_idx, target_tab_idx);
            self.state
                .record_pane_focus_change(previous_focus, ws_idx, new_pane.pane_id);
            self.state.mode = crate::app::Mode::Terminal;
        }
        self.terminal_runtimes
            .insert(new_pane.terminal.id.clone(), new_pane.runtime);
        self.state
            .remove_alias_shadowed_by_new_pane(new_pane.pane_id);
        self.state
            .terminals
            .insert(new_pane.terminal.id.clone(), new_pane.terminal);
        self.schedule_session_save();
        let pane = self.pane_info(ws_idx, new_pane.pane_id).unwrap();
        self.emit_event(EventEnvelope {
            event: EventKind::PaneCreated,
            data: EventData::PaneCreated { pane: pane.clone() },
        });
        self.emit_layout_updated_event(ws_idx, target_tab_idx);

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

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

    pub(super) fn handle_pane_scroll(&mut self, id: String, params: PaneScrollParams) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        runtime.set_scroll_offset_from_bottom(
            usize::try_from(params.offset_from_bottom).unwrap_or(usize::MAX),
        );
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(crate) fn pane_selection_text(
        &self,
        params: &PaneSelectionReadParams,
    ) -> Result<String, (&'static str, String)> {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return Err((
                "pane_not_found",
                format!("pane not found: {}", params.pane_id),
            ));
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return Err((
                "pane_not_found",
                format!("pane not found: {}", params.pane_id),
            ));
        };
        let before = runtime.content_seq();
        if params
            .content_revision
            .is_some_and(|revision| revision != before || !before.is_multiple_of(2))
        {
            return Err(("stale_content", "pane content changed".to_owned()));
        }
        let selection = crate::selection::Selection::absolute_range(
            pane_id,
            (params.anchor.row, params.anchor.col),
            (params.cursor.row, params.cursor.col),
        );
        let Some(text) = runtime.extract_selection(&selection) else {
            return Err((
                "selection_unavailable",
                "selection text is unavailable".to_owned(),
            ));
        };
        if params.content_revision.is_some() && runtime.content_seq() != before {
            return Err(("stale_content", "pane content changed".to_owned()));
        }
        Ok(text)
    }

    pub(super) fn handle_pane_selection_read(
        &mut self,
        id: String,
        params: PaneSelectionReadParams,
    ) -> String {
        match self.pane_selection_text(&params) {
            Ok(text) => encode_success(
                id,
                ResponseResult::PaneSelection {
                    pane_id: params.pane_id,
                    text,
                },
            ),
            Err((code, message)) => encode_error(id, code, message),
        }
    }

    pub(super) fn handle_pane_copy_motion(
        &mut self,
        id: String,
        params: PaneCopyMotionParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let before = runtime.content_seq();
        if params
            .content_revision
            .is_some_and(|revision| revision != before || !before.is_multiple_of(2))
        {
            return encode_error(id, "stale_content", "pane content changed");
        }
        let target = match params.motion {
            PaneCopyMotion::LineEnd | PaneCopyMotion::FirstNonBlank => {
                let width = runtime
                    .terminal_dimensions()
                    .map_or(1, |(cols, _)| cols.max(1));
                let selection = crate::selection::Selection::absolute_range(
                    pane_id,
                    (params.cursor.row, 0),
                    (params.cursor.row, width.saturating_sub(1)),
                );
                let Some(text) = runtime.extract_selection(&selection) else {
                    return encode_error(
                        id,
                        "copy_motion_unavailable",
                        "terminal row is unavailable",
                    );
                };
                let col = match params.motion {
                    PaneCopyMotion::LineEnd => {
                        crate::copy_mode::last_character_col(&text).unwrap_or(0)
                    }
                    PaneCopyMotion::FirstNonBlank => {
                        crate::copy_mode::first_non_blank_col(&text).unwrap_or(0)
                    }
                    _ => unreachable!(),
                };
                crate::pane::TerminalTextPoint {
                    row: params.cursor.row,
                    col: col.min(width.saturating_sub(1)),
                }
            }
            PaneCopyMotion::NextWordStart
            | PaneCopyMotion::PreviousWordStart
            | PaneCopyMotion::NextWordEnd
            | PaneCopyMotion::NextBigWordStart
            | PaneCopyMotion::PreviousBigWordStart
            | PaneCopyMotion::NextBigWordEnd => {
                let motion = match params.motion {
                    PaneCopyMotion::NextWordStart => crate::pane::TerminalWordMotion::NextStart,
                    PaneCopyMotion::PreviousWordStart => {
                        crate::pane::TerminalWordMotion::PreviousStart
                    }
                    PaneCopyMotion::NextWordEnd => crate::pane::TerminalWordMotion::NextEnd,
                    PaneCopyMotion::NextBigWordStart => {
                        crate::pane::TerminalWordMotion::NextBigStart
                    }
                    PaneCopyMotion::PreviousBigWordStart => {
                        crate::pane::TerminalWordMotion::PreviousBigStart
                    }
                    PaneCopyMotion::NextBigWordEnd => crate::pane::TerminalWordMotion::NextBigEnd,
                    _ => unreachable!(),
                };
                runtime
                    .word_motion_target(params.cursor.row, params.cursor.col, motion)
                    .unwrap_or(crate::pane::TerminalTextPoint {
                        row: params.cursor.row,
                        col: params.cursor.col,
                    })
            }
            PaneCopyMotion::PreviousParagraph | PaneCopyMotion::NextParagraph => runtime
                .paragraph_motion_target(
                    params.cursor.row,
                    if params.motion == PaneCopyMotion::PreviousParagraph {
                        -1
                    } else {
                        1
                    },
                )
                .map(|target| crate::pane::TerminalTextPoint {
                    row: target.row,
                    col: params.cursor.col,
                })
                .unwrap_or(crate::pane::TerminalTextPoint {
                    row: params.cursor.row,
                    col: params.cursor.col,
                }),
        };
        let after = runtime.content_seq();
        if params.content_revision.is_some() && after != before {
            return encode_error(id, "stale_content", "pane content changed");
        }
        encode_success(
            id,
            ResponseResult::PaneCopyMotion {
                pane_id: params.pane_id,
                cursor: crate::api::schema::PaneTextPoint {
                    row: target.row,
                    col: target.col,
                },
                content_revision: after,
            },
        )
    }

    pub(super) fn handle_pane_copy_search(
        &mut self,
        id: String,
        params: PaneCopySearchParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        const MAX_QUERY_BYTES: usize = 4096;
        const MAX_RETURNED_MATCHES: usize = 1024;
        if params.query.len() > MAX_QUERY_BYTES {
            return encode_error(id, "query_too_large", "copy search query is too large");
        }
        let before = runtime.content_seq();
        if before != params.content_revision || !before.is_multiple_of(2) {
            return encode_error(id, "stale_content", "pane content changed");
        }
        let cursor = crate::pane::TerminalTextPoint {
            row: params.cursor.row,
            col: params.cursor.col,
        };
        let previous = params.previous.map(|previous| {
            (
                crate::pane::TerminalTextPoint {
                    row: previous.start.row,
                    col: previous.start.col,
                },
                crate::pane::TerminalTextPoint {
                    row: previous.end.row,
                    col: previous.end.col,
                },
            )
        });
        let direction = match params.direction {
            PaneCopySearchDirection::Forward => crate::pane::TerminalSearchDirection::Forward,
            PaneCopySearchDirection::Backward => crate::pane::TerminalSearchDirection::Backward,
        };
        let result = runtime.search_text_window(
            &params.query,
            params.query.chars().any(char::is_uppercase),
            direction,
            cursor,
            previous,
            MAX_RETURNED_MATCHES,
        );
        let after = runtime.content_seq();
        if after != before || !after.is_multiple_of(2) {
            return encode_error(id, "stale_content", "pane content changed");
        }
        let matches = result
            .matches
            .into_iter()
            .map(|text_match| PaneTextRange {
                start: PaneTextPoint {
                    row: text_match.start.row,
                    col: text_match.start.col,
                },
                end: PaneTextPoint {
                    row: text_match.end.row,
                    col: text_match.end.col,
                },
            })
            .collect();
        encode_success(
            id,
            ResponseResult::PaneCopySearch {
                pane_id: params.pane_id,
                content_revision: after,
                matches,
                total: u64::try_from(result.total).unwrap_or(u64::MAX),
                current: result.current.and_then(|index| u32::try_from(index).ok()),
                current_global: result
                    .current_global
                    .and_then(|index| u64::try_from(index).ok()),
            },
        )
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
        self.state.mode = crate::app::Mode::Terminal;

        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };
        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_layout(&mut self, id: String, params: PaneLayoutParams) -> String {
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

    pub(super) fn handle_pane_focus_direction(
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
            self.state.mode = crate::app::Mode::Terminal;
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

    pub(super) fn handle_pane_resize(&mut self, id: String, params: PaneResizeParams) -> String {
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

    pub(super) fn handle_pane_swap(&mut self, id: String, params: PaneSwapParams) -> String {
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
            let Some(source_raw) = params.source_pane_id.as_deref() else {
                return encode_error(id, "invalid_pane_swap", "missing source_pane_id");
            };
            let Some(target_raw) = params.target_pane_id.as_deref() else {
                return encode_error(id, "invalid_pane_swap", "missing target_pane_id");
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
                return encode_error(id, "pane_layout_unavailable", "pane layout unavailable");
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
            (ws_idx, tab_idx, source_pane_id, target_pane_id, reason)
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

    pub(super) fn handle_pane_zoom(&mut self, id: String, params: PaneZoomParams) -> String {
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
        self.state.mode = crate::app::Mode::Terminal;
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

    pub(super) fn handle_pane_input_set(
        &mut self,
        id: String,
        params: PaneInputSetParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(pane) = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .and_then(|workspace| workspace.pane_state_mut(pane_id))
        else {
            return pane_not_found(id, &params.pane_id);
        };
        pane.right_click_passthrough = matches!(
            params.right_click,
            crate::api::schema::PaneRightClickTarget::Pane
        );
        encode_success(id, ResponseResult::Ok {})
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

    pub(super) fn handle_pane_read(&mut self, id: String, params: PaneReadParams) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(tab_idx) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.find_tab_index_for_pane(pane_id))
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let snapshot = crate::app::api_helpers::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: public_pane_id,
                    workspace_id,
                    tab_id: self.public_tab_id(ws_idx, tab_idx).unwrap(),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: snapshot.revision,
                    truncated: snapshot.truncated,
                    viewport_rows: snapshot.viewport_rows,
                    viewport_columns: snapshot.viewport_columns,
                    requested_lines: snapshot.requested_lines,
                    returned_lines: snapshot.returned_lines,
                    available_lines: snapshot.available_lines,
                    exhausted: snapshot.exhausted,
                },
            },
        )
    }

    pub(super) fn handle_pane_report_agent_session(
        &mut self,
        id: String,
        params: PaneReportAgentSessionParams,
    ) -> String {
        let Some((_ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(agent_label) = normalize_reported_agent_label(&params.agent) else {
            return invalid_agent(id);
        };
        self.handle_internal_event(crate::events::AppEvent::AgentSessionReported {
            pane_id,
            session_ref: crate::agent_resume::session_ref_from_report(
                &params.source,
                &agent_label,
                params.agent_session_id,
                params.agent_session_path,
            ),
            source: params.source,
            agent_label,
            seq: params.seq,
            session_start_source: crate::agent_resume::normalize_session_start_source(
                params.session_start_source,
            ),
        });

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_send_text(
        &mut self,
        id: String,
        params: PaneSendTextParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(params.text)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_send_input(
        &mut self,
        id: String,
        params: PaneSendInputParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let bytes = match super::super::api_helpers::encode_api_input(
            runtime,
            &params.text,
            &params.keys,
        ) {
            Ok(bytes) => bytes,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
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
        params: crate::api::schema::PaneCloseIfIdentityParams,
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

    pub(super) fn handle_pane_send_keys(
        &mut self,
        id: String,
        params: PaneSendKeysParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let encoded_keys = match encode_api_keys(runtime, &params.keys) {
            Ok(encoded_keys) => encoded_keys,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        for bytes in encoded_keys {
            if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
                return encode_error(id, "pane_send_failed", err.to_string());
            }
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

fn pane_not_found(id: String, pane_id: &str) -> String {
    encode_error(id, "pane_not_found", format!("pane {pane_id} not found"))
}

impl App {
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

    fn directional_pane_target(
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

    pub(super) fn pane_layout_snapshot(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<PaneLayoutSnapshot> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.get(tab_idx)?;
        let area = self.state.view.terminal_area;
        let focused_pane_id = self.public_pane_id(ws_idx, tab.layout.focused())?;
        let panes = crate::ui::apply_pane_chrome(
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
                        crate::api::schema::SplitDirection::Right
                    }
                    ratatui::layout::Direction::Vertical => {
                        crate::api::schema::SplitDirection::Down
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

    pub(super) fn emit_layout_updated_snapshot(&mut self, layout: PaneLayoutSnapshot) {
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

fn split_direction_to_layout(
    direction: crate::api::schema::SplitDirection,
) -> ratatui::layout::Direction {
    match direction {
        crate::api::schema::SplitDirection::Right => ratatui::layout::Direction::Horizontal,
        crate::api::schema::SplitDirection::Down => ratatui::layout::Direction::Vertical,
    }
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

fn split_path_id(idx: usize, path: &[bool]) -> String {
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

fn invalid_agent(id: String) -> String {
    encode_error(id, "invalid_agent", "agent label must not be empty")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::schema::{ErrorResponse, SuccessResponse},
        config::Config,
        detect::{Agent, AgentState},
        workspace::Workspace,
    };

    fn app_with_test_workspace() -> (App, String) {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("metadata")];
        app.state.ensure_test_terminals();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
        (app, public_pane_id)
    }

    #[test]
    fn api_pane_report_agent_session_preserves_session_identity_and_detected_state() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Working);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "session-report".into(),
            method: crate::api::schema::Method::PaneReportAgentSession(
                PaneReportAgentSessionParams {
                    pane_id: public_pane_id,
                    source: "herdr:codex".into(),
                    agent: "  CODEX  ".into(),
                    seq: Some(1),
                    agent_session_id: Some("codex-session".into()),
                    agent_session_path: None,
                    session_start_source: Some(" startup ".into()),
                },
            ),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "session-report");
        assert_eq!(success.result, ResponseResult::Ok {});
        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(terminal.state, AgentState::Working);
        let session = terminal.persisted_agent_session.as_ref().unwrap();
        assert_eq!(session.source, "herdr:codex");
        assert_eq!(session.agent, "codex");
        assert_eq!(session.session_ref.value, "codex-session");
        assert_eq!(
            session.session_ref.kind,
            crate::agent_resume::AgentSessionRefKind::Id
        );
    }

    #[test]
    fn api_pane_report_agent_session_rejects_missing_panes_and_empty_agents() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
        for (target, agent, expected_code) in [
            ("missing-pane".to_owned(), "codex", "pane_not_found"),
            (public_pane_id, " \t\n ", "invalid_agent"),
        ] {
            let response = app.handle_api_request(crate::api::schema::Request {
                id: "invalid-session-report".into(),
                method: crate::api::schema::Method::PaneReportAgentSession(
                    PaneReportAgentSessionParams {
                        pane_id: target,
                        source: "herdr:codex".into(),
                        agent: agent.into(),
                        seq: Some(1),
                        agent_session_id: Some("codex-session".into()),
                        agent_session_path: None,
                        session_start_source: Some("startup".into()),
                    },
                ),
            });
            let error: ErrorResponse = serde_json::from_str(&response).unwrap();
            assert_eq!(error.id, "invalid-session-report");
            assert_eq!(error.error.code, expected_code);
            assert!(app.state.terminals[&terminal_id]
                .persisted_agent_session
                .is_none());
        }
    }

    fn guarded_close_fixture() -> (
        App,
        crate::api::schema::PaneCloseIfIdentityParams,
        PaneId,
        PaneId,
    ) {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let other = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.ensure_test_terminals();
        let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.begin_managed_agent(
            "bus-r1-a2".into(),
            Agent::Codex,
            std::time::Instant::now(),
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(10),
        );
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Working);
        let params = crate::api::schema::PaneCloseIfIdentityParams {
            pane_id: public_pane_id,
            expected_terminal_id: terminal_id.to_string(),
            expected_agent: "codex".into(),
            expected_managed_name: "bus-r1-a2".into(),
            expected_session_id: None,
        };
        (app, params, pane_id, other)
    }

    #[test]
    fn guarded_close_rejects_changed_ownership_before_stopping_any_terminal() {
        let (mut app, params, pane_id, other) = guarded_close_fixture();
        let count = app.state.terminals.len();
        for field in ["pane", "terminal", "name", "provider", "session"] {
            let mut wrong = params.clone();
            match field {
                "pane" => wrong.pane_id = app.public_pane_id(0, other).unwrap(),
                "terminal" => {
                    wrong.expected_terminal_id = app
                        .state
                        .terminal_id_for_pane(0, other)
                        .unwrap()
                        .to_string()
                }
                "name" => wrong.expected_managed_name = "different-launch".into(),
                "provider" => wrong.expected_agent = "claude".into(),
                "session" => wrong.expected_session_id = Some("rebound-session".into()),
                _ => unreachable!(),
            }
            let response = app.handle_pane_close_if_identity("guard".into(), wrong);
            let error: ErrorResponse = serde_json::from_str(&response).unwrap();
            assert_eq!(error.error.code, "terminal_identity_changed", "{field}");
            assert_eq!(app.state.terminals.len(), count);
            assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
            assert!(app.state.workspaces[0].pane_state(other).is_some());
        }
    }

    #[tokio::test]
    async fn guarded_close_removes_only_exact_terminal_and_retries_absence_safely() {
        let (mut app, params, pane_id, other) = guarded_close_fixture();
        let other_terminal = app.state.terminal_id_for_pane(0, other).unwrap();
        let (runtime, _rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 2,
            );
        app.terminal_runtimes
            .insert(app.state.terminal_id_for_pane(0, pane_id).unwrap(), runtime);
        let response = app.handle_pane_close_if_identity("close".into(), params.clone());
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(success.result, ResponseResult::Ok {}));
        assert!(app.state.workspaces[0].pane_state(pane_id).is_none());
        assert!(app.state.terminals.contains_key(&other_terminal));
        assert!(!app
            .terminal_runtimes
            .contains_id(&params.expected_terminal_id));
        // A different pane remains safe even if supplied in a retry after the
        // expected terminal was already stopped and removed.
        let mut retry = params;
        retry.pane_id = app.public_pane_id(0, other).unwrap();
        let response = app.handle_pane_close_if_identity("retry".into(), retry);
        assert!(serde_json::from_str::<SuccessResponse>(&response).is_ok());
        assert!(app.state.workspaces[0].pane_state(other).is_some());
    }

    #[test]
    fn guarded_close_refuses_a_terminal_still_attached_to_multiple_panes() {
        let (mut app, params, pane_id, other) = guarded_close_fixture();
        let terminal = app.state.terminal_id_for_pane(0, pane_id).unwrap();
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&other)
            .unwrap()
            .attached_terminal_id = terminal;
        let response = app.handle_pane_close_if_identity("shared".into(), params);
        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "terminal_identity_changed");
        assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
        assert!(app.state.workspaces[0].pane_state(other).is_some());
    }

    #[test]
    fn pane_input_set_changes_only_the_target_pane() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let target = app.state.workspaces[0].tabs[0].root_pane;
        let other = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);

        let response = app.handle_pane_input_set(
            "req".into(),
            PaneInputSetParams {
                pane_id: public_pane_id,
                right_click: crate::api::schema::PaneRightClickTarget::Pane,
            },
        );

        let response: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(response.result, ResponseResult::Ok {}));
        assert!(
            app.state.workspaces[0]
                .pane_state(target)
                .unwrap()
                .right_click_passthrough
        );
        assert!(
            !app.state.workspaces[0]
                .pane_state(other)
                .unwrap()
                .right_click_passthrough
        );
    }

    fn app_with_send_key_runtime(
        capacity: usize,
    ) -> (App, String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let (runtime, rx) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, capacity);
        app.state.insert_test_runtime(pane_id, runtime);
        (app, public_pane_id, rx)
    }

    fn app_with_scrollback_runtime() -> (App, String, PaneId) {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let lines = (0..20)
            .map(|line| format!("line {line:02}\n"))
            .collect::<String>();
        let runtime = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            20,
            5,
            1000,
            lines.as_bytes(),
        );
        app.state.insert_test_runtime(pane_id, runtime);
        (app, public_pane_id, pane_id)
    }

    #[tokio::test]
    async fn api_pane_send_keys_accepts_control_navigation_chords() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(4);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
                pane_id,
                keys: vec![
                    "ctrl+h".into(),
                    "ctrl+j".into(),
                    "ctrl+k".into(),
                    "ctrl+l".into(),
                ],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x08]));
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0a]));
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0b]));
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0c]));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_keys_encodes_shift_tab_as_backtab() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
                pane_id,
                keys: vec!["shift+tab".into()],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from_static(b"\x1b[Z"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_get_exposes_scroll_metrics() {
        let (mut app, public_pane_id, pane_id) = app_with_scrollback_runtime();
        let runtime = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.scroll_up(3);

        let response = app.handle_pane_get(
            "req".into(),
            PaneTarget {
                pane_id: public_pane_id,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneInfo { pane } = success.result else {
            panic!("expected pane info response");
        };
        let scroll = pane.scroll.expect("scroll metrics");
        assert_eq!(scroll.offset_from_bottom, 3);
        assert!(scroll.max_offset_from_bottom >= scroll.offset_from_bottom);
        assert_eq!(scroll.viewport_rows, 5);
    }

    #[tokio::test]
    async fn api_pane_scroll_sets_and_clamps_endpoint_owned_history() {
        let (mut app, public_pane_id, pane_id) = app_with_scrollback_runtime();
        let runtime = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        let max_offset = runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .max_offset_from_bottom;

        let response = app.handle_pane_scroll(
            "req".into(),
            PaneScrollParams {
                pane_id: public_pane_id,
                offset_from_bottom: u64::MAX,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneInfo { pane } = success.result else {
            panic!("expected pane info response");
        };
        assert_eq!(
            pane.scroll.expect("scroll metrics").offset_from_bottom,
            max_offset as u64
        );
    }

    #[tokio::test]
    async fn api_pane_selection_read_uses_endpoint_terminal_text() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                5,
                1000,
                b"hello world",
            ),
        );

        let runtime = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .unwrap();
        let revision = runtime.content_seq();
        runtime.test_process_pty_bytes(b"\r\nagent is still working");
        assert_ne!(runtime.content_seq(), revision);
        let mut params = PaneSelectionReadParams {
            pane_id: public_pane_id.clone(),
            anchor: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
            cursor: crate::api::schema::PaneTextPoint { row: 0, col: 4 },
            content_revision: Some(revision),
        };
        assert_eq!(
            app.pane_selection_text(&params).unwrap_err().0,
            "stale_content"
        );
        params.content_revision = None;
        let response = app.handle_pane_selection_read("req".into(), params);

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            success.result,
            ResponseResult::PaneSelection {
                pane_id: public_pane_id,
                text: "hello".into(),
            }
        );
    }

    #[tokio::test]
    async fn api_copy_motion_uses_endpoint_terminal_word_semantics() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                5,
                1000,
                b"hello world",
            ),
        );

        let response = app.handle_pane_copy_motion(
            "req".into(),
            PaneCopyMotionParams {
                pane_id: public_pane_id.clone(),
                cursor: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
                motion: PaneCopyMotion::NextWordStart,
                content_revision: None,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            success.result,
            ResponseResult::PaneCopyMotion {
                pane_id: public_pane_id,
                cursor: crate::api::schema::PaneTextPoint { row: 0, col: 6 },
                content_revision: 0,
            }
        );
    }

    #[tokio::test]
    async fn api_paragraph_motion_preserves_the_copy_cursor_column() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                5,
                1000,
                b"one\r\n\r\nthree",
            ),
        );
        let response = app.handle_pane_copy_motion(
            "req".into(),
            PaneCopyMotionParams {
                pane_id: public_pane_id.clone(),
                cursor: PaneTextPoint { row: 0, col: 2 },
                motion: PaneCopyMotion::NextParagraph,
                content_revision: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            success.result,
            ResponseResult::PaneCopyMotion {
                pane_id: public_pane_id,
                cursor: PaneTextPoint { row: 1, col: 2 },
                content_revision: 0,
            }
        );
    }

    #[tokio::test]
    async fn api_copy_search_uses_endpoint_terminal_matches_and_wraps() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                5,
                1000,
                b"alpha beta alpha",
            ),
        );

        let content_revision = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .expect("runtime")
            .content_seq();
        let response = app.handle_pane_copy_search(
            "req".into(),
            PaneCopySearchParams {
                pane_id: public_pane_id.clone(),
                query: "alpha".into(),
                direction: PaneCopySearchDirection::Forward,
                cursor: PaneTextPoint { row: 0, col: 0 },
                content_revision,
                previous: None,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneCopySearch {
            pane_id,
            matches,
            current,
            total,
            current_global,
            ..
        } = success.result
        else {
            panic!("expected copy search response");
        };
        assert_eq!(pane_id, public_pane_id);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start, PaneTextPoint { row: 0, col: 0 });
        assert_eq!(matches[1].start, PaneTextPoint { row: 0, col: 11 });
        assert_eq!(current, Some(1));
        assert_eq!(current_global, Some(1));
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn api_copy_search_bounds_returned_matches_but_keeps_exact_total() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let text = "a ".repeat(1500);
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                200,
                20,
                4000,
                text.as_bytes(),
            ),
        );
        let content_revision = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .expect("runtime")
            .content_seq();

        let response = app.handle_pane_copy_search(
            "req".into(),
            PaneCopySearchParams {
                pane_id: public_pane_id,
                query: "a".into(),
                direction: PaneCopySearchDirection::Forward,
                cursor: PaneTextPoint { row: 0, col: 0 },
                content_revision,
                previous: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneCopySearch { matches, total, .. } = success.result else {
            panic!("expected copy search response");
        };
        assert_eq!(total, 1500);
        assert_eq!(matches.len(), 1024);
    }

    #[tokio::test]
    async fn api_copy_search_rejects_stale_content_revision() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                5,
                1000,
                b"alpha beta",
            ),
        );
        let response = app.handle_pane_copy_search(
            "req".into(),
            PaneCopySearchParams {
                pane_id: public_pane_id,
                query: "alpha".into(),
                direction: PaneCopySearchDirection::Forward,
                cursor: PaneTextPoint { row: 0, col: 0 },
                content_revision: 2,
                previous: None,
            },
        );
        assert!(response.contains("stale_content"));
    }

    #[tokio::test]
    async fn api_pane_read_reports_when_older_rows_are_omitted() {
        let (mut app, public_pane_id, _pane_id) = app_with_scrollback_runtime();

        let response = app.handle_pane_read(
            "req".into(),
            PaneReadParams {
                pane_id: public_pane_id,
                source: crate::api::schema::ReadSource::Recent,
                lines: Some(2),
                format: crate::api::schema::ReadFormat::Text,
                strip_ansi: true,
                intent: crate::api::schema::ReadIntent::Interactive,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneRead { read } = success.result else {
            panic!("expected pane read response");
        };
        assert!(read.text.contains("line 19"));
        assert!(read.truncated);
    }

    #[tokio::test]
    async fn api_pane_read_honors_recent_line_requests_above_one_thousand() {
        let (mut app, public_pane_id) = app_with_test_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let runtime =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(80, 3, 10_000_000, &[]);
        for index in 0..1500 {
            runtime.test_process_pty_bytes(format!("{index:06}\r\n").as_bytes());
        }
        app.state.insert_test_runtime(pane_id, runtime);

        let response = app.handle_pane_read(
            "req".into(),
            PaneReadParams {
                pane_id: public_pane_id,
                source: crate::api::schema::ReadSource::Recent,
                lines: Some(5000),
                format: crate::api::schema::ReadFormat::Text,
                strip_ansi: true,
                intent: crate::api::schema::ReadIntent::Interactive,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneRead { read } = success.result else {
            panic!("expected pane read response");
        };
        let returned = read
            .text
            .split_inclusive('\n')
            .filter(|line| !line.is_empty())
            .count();
        assert!(returned > 1000, "got {returned} rows");
        assert!(read.text.contains("000000"));
        assert!(!read.truncated);
    }

    #[tokio::test]
    async fn api_pane_send_keys_preserves_legacy_control_c_aliases() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(3);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
                pane_id,
                keys: vec!["C-c".into(), "c-c".into(), "ctrl+c".into()],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x03]));
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x03]));
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x03]));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_keys_accepts_literal_plus() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
                pane_id,
                keys: vec!["+".into()],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from_static(b"+"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_keys_sends_shifted_punctuation_as_text_in_kitty_mode() {
        let (mut app, pane_id) = app_with_test_workspace();
        let internal_pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                24,
                0,
                b"\x1b[>7u",
                1,
            );
        app.state.insert_test_runtime(internal_pane_id, runtime);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
                pane_id,
                keys: vec!["shift+?".into()],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from_static(b"?"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_input_brackets_text_and_enter_atomically() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);
        let internal_pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.lookup_runtime_sender(0, internal_pane_id)
            .unwrap()
            .test_process_pty_bytes(b"\x1b[?2004h");

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendInput(PaneSendInputParams {
                pane_id,
                text: "A != B".into(),
                keys: vec!["Enter".into()],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(
            rx.try_recv().unwrap(),
            bytes::Bytes::from_static(b"\x1b[200~A != B\x1b[201~\r")
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_input_keys_accept_key_combo_chords() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendInput(PaneSendInputParams {
                pane_id,
                text: String::new(),
                keys: vec!["ctrl+j".into()],
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0a]));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_keys_rejects_invalid_keys_before_writing() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(2);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
                pane_id,
                keys: vec!["ctrl+h".into(), "not-a-key".into()],
            }),
        });

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "invalid_key");
        assert_eq!(error.error.message, "unsupported key not-a-key");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn api_pane_send_input_rejects_prefix_bindings_before_writing_text_or_keys() {
        let (mut app, pane_id, mut rx) = app_with_send_key_runtime(4);
        let raw_key = " prefix+h ".to_string();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneSendInput(PaneSendInputParams {
                pane_id,
                text: "hello".into(),
                keys: vec!["ctrl+h".into(), raw_key.clone()],
            }),
        });

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "invalid_key");
        assert_eq!(error.error.message, format!("unsupported key {raw_key}"));
        assert!(rx.try_recv().is_err());
    }

    fn app_with_one_workspace() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("issue")];
        app.state.ensure_test_terminals();
        app
    }

    #[test]
    fn api_pane_close_of_last_pane_closes_its_workspace() {
        let mut app = app_with_one_workspace();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let response = app.handle_pane_close(
            "req".into(),
            PaneTarget {
                pane_id: public_pane_id,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert!(app.state.workspaces.is_empty());
    }

    #[test]
    fn api_pane_current_prefers_caller_pane_id() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.ensure_test_terminals();
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        let root_public = app.public_pane_id(0, root).unwrap();
        let right_public = app.public_pane_id(0, right).unwrap();

        let response = app.handle_pane_current(
            "req".into(),
            crate::api::schema::PaneCurrentParams {
                caller_pane_id: Some(right_public.clone()),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneCurrent { pane } = success.result else {
            panic!("expected pane current response");
        };
        assert_eq!(pane.pane_id, right_public);
        assert!(!pane.focused);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));
        assert_ne!(pane.pane_id, root_public);
    }

    #[test]
    fn api_pane_current_falls_back_to_focused_pane() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        let root_public = app.public_pane_id(0, root).unwrap();

        let response = app.handle_pane_current(
            "req".into(),
            crate::api::schema::PaneCurrentParams::default(),
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneCurrent { pane } = success.result else {
            panic!("expected pane current response");
        };
        assert_eq!(pane.pane_id, root_public);
        assert!(pane.focused);
    }

    #[test]
    fn api_pane_current_dispatches_through_socket_request() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let root_public = app.public_pane_id(0, root).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneCurrent(
                crate::api::schema::PaneCurrentParams::default(),
            ),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneCurrent { pane } = success.result else {
            panic!("expected pane current response");
        };
        assert_eq!(pane.pane_id, root_public);
    }

    #[test]
    fn api_pane_current_reports_invalid_caller_pane_id() {
        let mut app = app_with_one_workspace();

        let response = app.handle_pane_current(
            "req".into(),
            crate::api::schema::PaneCurrentParams {
                caller_pane_id: Some("missing".into()),
            },
        );

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "pane_not_found");
    }

    #[test]
    fn api_pane_current_reports_no_active_pane() {
        let mut app = app_with_one_workspace();
        app.state.active = None;

        let response = app.handle_pane_current(
            "req".into(),
            crate::api::schema::PaneCurrentParams::default(),
        );

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "pane_not_found");
    }

    #[test]
    fn api_pane_swap_explicit_source_and_target_preserves_focus_and_returns_layout() {
        let mut app = app_with_one_workspace();
        let source = app.state.workspaces[0].tabs[0].root_pane;
        let target = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(source);
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            ratatui::layout::Rect::new(0, 0, 100, 20),
        );
        let source_public = app.public_pane_id(0, source).unwrap();
        let target_public = app.public_pane_id(0, target).unwrap();

        let response = app.handle_pane_swap(
            "req".into(),
            PaneSwapParams {
                source_pane_id: Some(source_public.clone()),
                target_pane_id: Some(target_public.clone()),
                ..PaneSwapParams::default()
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneSwap { swap } = success.result else {
            panic!("expected pane swap response");
        };
        assert!(swap.changed);
        assert_eq!(swap.reason, None);
        assert_eq!(swap.source_pane_id, source_public);
        assert_eq!(swap.target_pane_id, Some(target_public));
        assert_eq!(swap.focused_pane_id, swap.source_pane_id);
        assert_eq!(swap.layout.focused_pane_id, swap.source_pane_id);
        assert_eq!(swap.layout.panes.len(), 2);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(source));
    }

    #[test]
    fn api_pane_swap_direction_no_neighbor_returns_unchanged_layout() {
        let mut app = app_with_one_workspace();
        let source = app.state.workspaces[0].tabs[0].root_pane;
        app.state.workspaces[0].tabs[0].layout.focus_pane(source);
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            ratatui::layout::Rect::new(0, 0, 100, 20),
        );
        let source_public = app.public_pane_id(0, source).unwrap();

        let response = app.handle_pane_swap(
            "req".into(),
            PaneSwapParams {
                pane_id: Some(source_public.clone()),
                direction: Some(PaneDirection::Left),
                ..PaneSwapParams::default()
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneSwap { swap } = success.result else {
            panic!("expected pane swap response");
        };
        assert!(!swap.changed);
        assert_eq!(swap.reason, Some(PaneSwapReason::NoNeighbor));
        assert_eq!(swap.source_pane_id, source_public);
        assert_eq!(swap.target_pane_id, None);
        assert_eq!(swap.layout.panes.len(), 1);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_pane_swap_explicit_missing_target_returns_not_found_noop() {
        let mut app = app_with_one_workspace();
        let source = app.state.workspaces[0].tabs[0].root_pane;
        let source_public = app.public_pane_id(0, source).unwrap();

        let response = app.handle_pane_swap(
            "req".into(),
            PaneSwapParams {
                source_pane_id: Some(source_public.clone()),
                target_pane_id: Some("missing-pane".into()),
                ..PaneSwapParams::default()
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneSwap { swap } = success.result else {
            panic!("expected pane swap response");
        };
        assert!(!swap.changed);
        assert_eq!(swap.reason, Some(PaneSwapReason::NotFound));
        assert_eq!(swap.source_pane_id, source_public);
        assert_eq!(swap.target_pane_id, Some("missing-pane".into()));
        assert_eq!(swap.layout.panes.len(), 1);
    }

    #[test]
    fn api_pane_swap_explicit_missing_source_returns_not_found_noop() {
        let mut app = app_with_one_workspace();
        let target = app.state.workspaces[0].tabs[0].root_pane;
        let target_public = app.public_pane_id(0, target).unwrap();

        let response = app.handle_pane_swap(
            "req".into(),
            PaneSwapParams {
                source_pane_id: Some("missing-pane".into()),
                target_pane_id: Some(target_public.clone()),
                ..PaneSwapParams::default()
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneSwap { swap } = success.result else {
            panic!("expected pane swap response");
        };
        assert!(!swap.changed);
        assert_eq!(swap.reason, Some(PaneSwapReason::NotFound));
        assert_eq!(swap.source_pane_id, "missing-pane");
        assert_eq!(swap.target_pane_id, Some(target_public));
        assert_eq!(swap.layout.panes.len(), 1);
    }

    #[test]
    fn api_pane_swap_explicit_cross_workspace_preserves_target_id() {
        let mut app = app_with_one_workspace();
        app.state.workspaces.push(Workspace::test_new("other"));
        let source = app.state.workspaces[0].tabs[0].root_pane;
        let target = app.state.workspaces[1].tabs[0].root_pane;
        let source_public = app.public_pane_id(0, source).unwrap();
        let target_public = app.public_pane_id(1, target).unwrap();

        let response = app.handle_pane_swap(
            "req".into(),
            PaneSwapParams {
                source_pane_id: Some(source_public.clone()),
                target_pane_id: Some(target_public.clone()),
                ..PaneSwapParams::default()
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneSwap { swap } = success.result else {
            panic!("expected pane swap response");
        };
        assert!(!swap.changed);
        assert_eq!(swap.reason, Some(PaneSwapReason::CrossTab));
        assert_eq!(swap.source_pane_id, source_public);
        assert_eq!(swap.target_pane_id, Some(target_public));
        assert_eq!(swap.layout.workspace_id, app.public_workspace_id(0));
    }

    #[test]
    fn api_pane_zoom_current_toggles_zoom() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let _right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        let root_public = app.public_pane_id(0, root).unwrap();

        let response = app.handle_pane_zoom("req".into(), PaneZoomParams::default());

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(zoom.changed);
        assert!(zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert_eq!(zoom.reason, None);
        assert_eq!(zoom.pane_id, root_public);
        assert_eq!(zoom.focused_pane_id, zoom.pane_id);
        assert!(zoom.zoomed);
        assert!(zoom.layout.zoomed);
        assert!(matches!(
            &app.event_hub.events_after(0).last().expect("layout event").1.data,
            EventData::LayoutUpdated { layout }
                if layout.tab_id == app.public_tab_id(0, 0).unwrap() && layout.zoomed
        ));

        let response = app.handle_pane_zoom("req".into(), PaneZoomParams::default());
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(zoom.changed);
        assert!(zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert!(!zoom.zoomed);
        assert!(!zoom.layout.zoomed);
        assert!(matches!(
            &app.event_hub.events_after(0).last().expect("layout event").1.data,
            EventData::LayoutUpdated { layout }
                if layout.tab_id == app.public_tab_id(0, 0).unwrap() && !layout.zoomed
        ));
    }

    #[test]
    fn api_pane_zoom_single_pane_returns_noop() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let root_public = app.public_pane_id(0, root).unwrap();

        let response = app.handle_pane_zoom(
            "req".into(),
            PaneZoomParams {
                pane_id: Some(root_public.clone()),
                mode: PaneZoomMode::Toggle,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(!zoom.changed);
        assert!(!zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert_eq!(zoom.reason, Some(PaneZoomReason::SinglePane));
        assert_eq!(zoom.pane_id, root_public);
        assert!(!zoom.zoomed);
        assert!(!app.state.workspaces[0].tabs[0].zoomed);
    }

    #[test]
    fn api_pane_zoom_on_and_off_are_idempotent() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let _right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        let root_public = app.public_pane_id(0, root).unwrap();

        let response = app.handle_pane_zoom(
            "req".into(),
            PaneZoomParams {
                pane_id: Some(root_public.clone()),
                mode: PaneZoomMode::On,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(zoom.changed);
        assert!(zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert!(zoom.zoomed);

        let response = app.handle_pane_zoom(
            "req".into(),
            PaneZoomParams {
                pane_id: Some(root_public.clone()),
                mode: PaneZoomMode::On,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(!zoom.changed);
        assert!(!zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert_eq!(zoom.reason, Some(PaneZoomReason::AlreadyZoomed));
        assert!(zoom.zoomed);

        let response = app.handle_pane_zoom(
            "req".into(),
            PaneZoomParams {
                pane_id: Some(root_public),
                mode: PaneZoomMode::Off,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(zoom.changed);
        assert!(zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert!(!zoom.zoomed);

        let response = app.handle_pane_zoom(
            "req".into(),
            PaneZoomParams {
                pane_id: None,
                mode: PaneZoomMode::Off,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(!zoom.changed);
        assert!(!zoom.zoom_changed);
        assert!(!zoom.focus_changed);
        assert_eq!(zoom.reason, Some(PaneZoomReason::AlreadyUnzoomed));
        assert!(!zoom.zoomed);
    }

    #[test]
    fn api_pane_zoom_idempotent_mode_reports_focus_change() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        app.state.workspaces[0].tabs[0].zoomed = true;
        let right_public = app.public_pane_id(0, right).unwrap();

        let response = app.handle_pane_zoom(
            "req".into(),
            PaneZoomParams {
                pane_id: Some(right_public),
                mode: PaneZoomMode::On,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneZoom { zoom } = success.result else {
            panic!("expected pane zoom response");
        };
        assert!(zoom.changed);
        assert!(!zoom.zoom_changed);
        assert!(zoom.focus_changed);
        assert_eq!(zoom.reason, Some(PaneZoomReason::AlreadyZoomed));
        assert!(zoom.zoomed);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
        assert!(matches!(
            &app.event_hub.events_after(0).last().expect("layout event").1.data,
            EventData::LayoutUpdated { layout }
                if layout.focused_pane_id == app.public_pane_id(0, right).unwrap()
        ));
    }

    #[test]
    fn api_pane_zoom_params_serialize_modes() {
        let request = crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::PaneZoom(PaneZoomParams {
                pane_id: Some("issue-1".into()),
                mode: PaneZoomMode::On,
            }),
        };

        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("\"method\":\"pane.zoom\""));
        assert!(encoded.contains("\"mode\":\"on\""));

        let decoded: crate::api::schema::Request = serde_json::from_str(&encoded).unwrap();
        let crate::api::schema::Method::PaneZoom(params) = decoded.method else {
            panic!("expected pane zoom request");
        };
        assert_eq!(params.pane_id, Some("issue-1".into()));
        assert_eq!(params.mode, PaneZoomMode::On);
    }

    #[test]
    fn api_pane_layout_returns_public_ids_rects_and_splits() {
        let mut app = app_with_one_workspace();
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            ratatui::layout::Rect::new(0, 0, 100, 20),
        );
        let root_public = app.public_pane_id(0, root).unwrap();
        let right_public = app.public_pane_id(0, right).unwrap();

        let response = app.handle_pane_layout(
            "req".into(),
            crate::api::schema::PaneLayoutParams {
                pane_id: Some(root_public.clone()),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneLayout { layout } = success.result else {
            panic!("expected pane layout response");
        };
        assert_eq!(layout.focused_pane_id, root_public);
        assert!(layout.panes.iter().any(|pane| pane.pane_id == root_public));
        assert!(layout.panes.iter().any(|pane| pane.pane_id == right_public));
        assert_eq!(layout.splits.len(), 1);
        assert_eq!(
            layout.splits[0].direction,
            crate::api::schema::SplitDirection::Right
        );
    }

    #[test]
    fn api_pane_resize_changes_target_ratio_without_changing_focus() {
        let mut app = app_with_one_workspace();
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(right);
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            ratatui::layout::Rect::new(0, 0, 100, 20),
        );
        let root_public = app.public_pane_id(0, root).unwrap();
        let right_public = app.public_pane_id(0, right).unwrap();

        let response = app.handle_pane_resize(
            "req".into(),
            crate::api::schema::PaneResizeParams {
                pane_id: Some(root_public.clone()),
                direction: PaneDirection::Right,
                amount: Some(0.1),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneResize { resize } = success.result else {
            panic!("expected pane resize response");
        };
        assert!(resize.changed);
        assert_eq!(resize.reason, None);
        assert_eq!(resize.pane_id, root_public);
        assert_eq!(resize.focused_pane_id, right_public);
        assert_eq!(resize.layout.focused_pane_id, right_public);
        assert!((resize.layout.splits[0].ratio - 0.6).abs() < f32::EPSILON);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
        assert!(matches!(
            &app.event_hub.events_after(0).last().expect("layout event").1.data,
            EventData::LayoutUpdated { layout }
                if layout.tab_id == app.public_tab_id(0, 0).unwrap()
                    && (layout.splits[0].ratio - 0.6).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn api_pane_focus_direction_focuses_neighbor() {
        let mut app = app_with_one_workspace();
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            ratatui::layout::Rect::new(0, 0, 100, 20),
        );
        let root_public = app.public_pane_id(0, root).unwrap();
        let right_public = app.public_pane_id(0, right).unwrap();

        let response = app.handle_pane_focus_direction(
            "req".into(),
            crate::api::schema::PaneFocusDirectionParams {
                pane_id: Some(root_public.clone()),
                direction: PaneDirection::Right,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneFocusDirection { focus } = success.result else {
            panic!("expected pane focus direction response");
        };
        assert!(focus.changed);
        assert_eq!(focus.reason, None);
        assert_eq!(focus.source_pane_id, root_public);
        assert_eq!(focus.focused_pane_id, Some(right_public.clone()));
        assert_eq!(focus.layout.focused_pane_id, right_public);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
    }

    #[test]
    fn api_pane_focus_focuses_direct_target_across_tabs_and_workspaces() {
        let mut app = app_with_one_workspace();
        app.state.workspaces.push(Workspace::test_new("other"));
        let target_tab_idx = app.state.workspaces[1].test_add_tab(Some("target"));
        app.state.workspaces[1].switch_tab(target_tab_idx);
        let target_pane = app.state.workspaces[1].tabs[target_tab_idx].root_pane;
        app.state.ensure_test_terminals();
        let target_public = app.public_pane_id(1, target_pane).unwrap();
        app.state.switch_workspace(0);
        assert_eq!(app.state.active, Some(0));

        let response = app.handle_pane_focus(
            "req".into(),
            crate::api::schema::PaneTarget {
                pane_id: target_public.clone(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneInfo { pane } = success.result else {
            panic!("expected pane info response");
        };
        assert_eq!(pane.pane_id, target_public);
        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.workspaces[1].active_tab, target_tab_idx);
        assert_eq!(app.state.workspaces[1].focused_pane_id(), Some(target_pane));
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn api_pane_focus_marks_already_focused_done_pane_seen() {
        let mut app = app_with_one_workspace();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.outer_terminal_focus = Some(false);

        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state.terminals.get_mut(&terminal_id).unwrap().state = crate::detect::AgentState::Idle;
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .seen = false;
        app.state.workspaces[0].tabs[0].layout.focus_pane(pane_id);

        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
        let response = app.handle_pane_focus(
            "req".into(),
            PaneTarget {
                pane_id: public_pane_id,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneInfo { pane } = success.result else {
            panic!("expected pane info response");
        };
        assert_eq!(pane.agent_status, crate::api::schema::AgentStatus::Idle);
    }

    #[test]
    fn api_pane_focus_rejects_invalid_pane_id() {
        let mut app = app_with_one_workspace();

        let response = app.handle_pane_focus(
            "req".into(),
            crate::api::schema::PaneTarget {
                pane_id: "pane_missing".into(),
            },
        );

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "pane_not_found");
    }

    #[test]
    fn api_pane_focus_direction_no_neighbor_is_noop() {
        let mut app = app_with_one_workspace();
        let root = app.state.workspaces[0].tabs[0].root_pane;
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            ratatui::layout::Rect::new(0, 0, 100, 20),
        );
        let root_public = app.public_pane_id(0, root).unwrap();

        let response = app.handle_pane_focus_direction(
            "req".into(),
            crate::api::schema::PaneFocusDirectionParams {
                pane_id: Some(root_public.clone()),
                direction: PaneDirection::Left,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneFocusDirection { focus } = success.result else {
            panic!("expected pane focus direction response");
        };
        assert!(!focus.changed);
        assert_eq!(focus.reason, Some(PaneFocusDirectionReason::NoNeighbor));
        assert_eq!(focus.source_pane_id, root_public.clone());
        assert_eq!(focus.focused_pane_id, Some(root_public));
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));
    }
}
