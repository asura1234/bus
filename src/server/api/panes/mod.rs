use bytes::Bytes;

use crate::protocol::api::schema::{
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
use crate::server::app::App;
#[cfg(test)]
use crate::server::app::Mode;
use crate::server::terminals::events::{PaneZoomCommand, PaneZoomNoopReason};
use crate::server::workspaces::layout::{find_in_direction, NavDirection, PaneId};

use super::errors::{encode_error, encode_success};
use super::input_encoding::{encode_api_keys, normalize_reported_agent_label};

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
                cursor: crate::protocol::api::schema::PaneTextPoint {
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
        self.state.mode = crate::server::app::Mode::Terminal;

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
            crate::protocol::api::schema::PaneRightClickTarget::Pane
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
        let snapshot = crate::server::api::input_encoding::read_terminal_snapshot(
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
        let bytes =
            match super::input_encoding::encode_api_input(runtime, &params.text, &params.keys) {
                Ok(bytes) => bytes,
                Err(key) => {
                    return encode_error(id, "invalid_key", format!("unsupported key {key}"))
                }
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
    direction: crate::protocol::api::schema::SplitDirection,
) -> ratatui::layout::Direction {
    match direction {
        crate::protocol::api::schema::SplitDirection::Right => {
            ratatui::layout::Direction::Horizontal
        }
        crate::protocol::api::schema::SplitDirection::Down => ratatui::layout::Direction::Vertical,
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
#[path = "tests/session_test.rs"]
mod tests;
