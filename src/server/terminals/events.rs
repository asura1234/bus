//! Pure state mutations on AppState.
//! These don't need channels, async, or PTY runtime.

use std::time::Instant;

use crate::detect::{Agent, AgentState};
use crate::events::AppEvent;
#[cfg(test)]
use crate::layout::NavDirection;
use crate::layout::PaneId;
use crate::selection::Selection;
use crate::terminal::{EffectiveStateChange, TerminalStateMutation};

use crate::server::api::input_encoding::pane_agent_status;
use crate::server::app_state::AppState;
#[cfg(test)]
use crate::server::app_state::{Mode, ToastKind};
use crate::server::notifications::policy::is_completion_transition;
pub use crate::server::notifications::policy::{
    active_tab_suppresses_notifications, notification_context,
    notification_toast_for_pane_state_update,
    notification_toast_for_state_change_with_agent_labels,
};
#[cfg(test)]
use crate::utils::text::hit_testing::word_bounds_at_column;
pub(crate) use crate::utils::text::hit_testing::{logical_cell_for_visible_cell, url_at_column};
pub(crate) use crate::utils::url::safe_web_url;

fn terminal_char_width(ch: char) -> u16 {
    u16::from(crate::utils::text::width::unicode_codepoint_width(
        ch as u32,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneStateUpdate {
    pub pane_id: PaneId,
    pub ws_idx: usize,
    pub previous_agent_label: Option<String>,
    pub previous_known_agent: Option<Agent>,
    pub previous_state: AgentState,
    pub previous_seen: bool,
    pub previous_presentation: crate::terminal::EffectivePresentation,
    pub agent_label: Option<String>,
    pub known_agent: Option<Agent>,
    pub state: AgentState,
    pub seen: bool,
    pub presentation: crate::terminal::EffectivePresentation,
    pub agent_name_changed: bool,
    pub agent_released: bool,
    pub agent_release_status: Option<crate::api::schema::AgentStatus>,
    pub suppress_completion: bool,
}

// ---------------------------------------------------------------------------
// Focus tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneZoomCommand {
    Toggle,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneZoomNoopReason {
    SinglePane,
    AlreadyZoomed,
    AlreadyUnzoomed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneZoomOutcome {
    pub changed: bool,
    pub focus_changed: bool,
    pub reason: Option<PaneZoomNoopReason>,
    pub zoomed: bool,
}

// ---------------------------------------------------------------------------
// Workspace operations
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Pane operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn url_at_pane_surface_cell(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        viewport_row: u16,
        col: u16,
    ) -> Option<String> {
        let rt = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)?;
        let (height, width) = rt.current_size();
        url_at_runtime_cell(
            rt,
            pane_id,
            ratatui::layout::Rect::new(0, 0, width, height),
            viewport_row,
            col,
            rt.scroll_metrics(),
        )
    }
}

fn url_at_runtime_cell(
    runtime: &crate::terminal::TerminalRuntime,
    pane_id: crate::layout::PaneId,
    area: ratatui::layout::Rect,
    viewport_row: u16,
    col: u16,
    metrics: Option<crate::pane::ScrollMetrics>,
) -> Option<String> {
    if viewport_row >= area.height || col >= area.width {
        return None;
    }
    let screen_col = area.x.saturating_add(col);
    let screen_row = area.y.saturating_add(viewport_row);
    if let Some((_, _, uri)) = runtime
        .visible_hyperlinks(area)
        .into_iter()
        .find(|((x, y), _, _)| *x == screen_col && *y == screen_row)
    {
        return Some(uri);
    }

    let visible_selection = Selection::line_range(
        pane_id,
        crate::selection::absolute_row_for_viewport(0, metrics),
        crate::selection::absolute_row_for_viewport(area.height.saturating_sub(1), metrics),
        area.width.saturating_sub(1),
    );
    let visible_text = runtime.extract_selection(&visible_selection)?;
    let logical_cell = logical_cell_for_visible_cell(
        &visible_text,
        area.width,
        viewport_row,
        col,
        terminal_char_width,
    )?;
    let line_start = visible_text[..logical_cell.byte_index]
        .rfind('\n')
        .map_or(0, |idx| idx + 1);
    let line_end = visible_text[logical_cell.byte_index..]
        .find('\n')
        .map_or(visible_text.len(), |idx| logical_cell.byte_index + idx);
    let line = visible_text.get(line_start..line_end)?;
    url_at_column(line, logical_cell.logical_col, terminal_char_width).map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

impl AppState {
    /// Recomputes automatic workspace labels whose identity cwd moved. Repository discovery
    /// only reads `.git` metadata from disk, so it runs inline when a terminal reports a cwd.
    pub fn refresh_workspace_auto_labels(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> bool {
        let terminals = &self.terminals;
        let mut changed = false;
        for ws in &mut self.workspaces {
            let Some(cwd) = ws.resolved_identity_cwd_from(terminals, terminal_runtimes) else {
                continue;
            };
            if ws.cached_identity_cwd == cwd {
                continue;
            }
            let label = crate::workspace::workspace_auto_label(&cwd);
            ws.cached_identity_cwd = cwd;
            if ws.cached_auto_label != label {
                ws.cached_auto_label = label;
                changed |= ws.custom_name.is_none();
            }
        }
        changed
    }

    pub fn handle_app_event(&mut self, event: AppEvent) -> Vec<PaneStateUpdate> {
        match event {
            AppEvent::PaneDied { pane_id, .. } => {
                self.handle_pane_died(pane_id);
                Vec::new()
            }
            AppEvent::AgentProcessDetected {
                pane_id,
                agent,
                observed_at,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    Some(terminal.set_detected_agent_process_at(agent, observed_at))
                })
                .into_iter()
                .collect(),
            AppEvent::StateChanged {
                pane_id,
                agent,
                state,
                visible_blocker,
                process_exited,
                observed_at,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    Some(terminal.set_detected_state_with_screen_signals_at(
                        agent,
                        state,
                        visible_blocker,
                        process_exited,
                        observed_at,
                    ))
                })
                .into_iter()
                .collect(),
            AppEvent::AgentSessionReported {
                pane_id,
                source,
                agent_label,
                seq,
                session_ref,
                session_start_source,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    terminal.set_agent_session_ref_for_session_start(
                        source,
                        agent_label,
                        session_ref,
                        seq,
                        session_start_source,
                    )
                })
                .into_iter()
                .collect(),
            // Host-local effects are intercepted by HeadlessServer and forwarded to the
            // foreground client; they never touch AppState. Kept for AppEvent exhaustiveness.
            AppEvent::TerminalBell { .. } => Vec::new(),
            AppEvent::ClipboardWrite { .. } => Vec::new(),
            AppEvent::TerminalCwdReported { pane_id, cwd } => {
                if !cwd.is_absolute() || !cwd.is_dir() {
                    return Vec::new();
                }
                let Some(terminal_id) = self.workspaces.iter().find_map(|ws| {
                    ws.pane_state(pane_id)
                        .map(|pane| pane.attached_terminal_id.clone())
                }) else {
                    return Vec::new();
                };
                let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
                    return Vec::new();
                };
                if terminal.cwd != cwd {
                    terminal.cwd = cwd;
                    self.mark_session_dirty();
                }
                Vec::new()
            }
        }
    }

    fn update_terminal_state<F>(&mut self, pane_id: PaneId, update: F) -> Option<PaneStateUpdate>
    where
        F: FnOnce(&mut crate::terminal::TerminalState) -> Option<TerminalStateMutation>,
    {
        self.update_terminal_state_with_completion_policy(pane_id, false, update)
    }

    fn update_terminal_state_with_completion_policy<F>(
        &mut self,
        pane_id: PaneId,
        force_suppress_completion: bool,
        update: F,
    ) -> Option<PaneStateUpdate>
    where
        F: FnOnce(&mut crate::terminal::TerminalState) -> Option<TerminalStateMutation>,
    {
        let ws_idx = self
            .workspaces
            .iter()
            .position(|ws| ws.pane_state(pane_id).is_some())?;
        let terminal_id = self.workspaces[ws_idx]
            .pane_state(pane_id)?
            .attached_terminal_id
            .clone();
        let previous_seen = self.workspaces[ws_idx].pane_state(pane_id)?.seen;
        let now = Instant::now();
        let (
            mutation,
            managed_changed,
            agent_name_changed,
            unchanged_change,
            managed_launch_pending,
            suppress_acquisition_completion,
        ) = {
            let terminal = self.terminals.get_mut(&terminal_id)?;
            let previous_agent_name = terminal.agent_name.clone();
            let managed_launch_pending = terminal.managed_agent_launch_pending();
            let mutation = update(terminal)?;
            let managed_changed = terminal.reconcile_managed_agent_at(now, false);
            let suppress_acquisition_completion = terminal.finish_agent_process_acquisition();
            let agent_name_changed = terminal.agent_name != previous_agent_name;
            let unchanged_change = (mutation.agent_released || agent_name_changed)
                .then(|| terminal.unchanged_effective_state_change_at(now));
            (
                mutation,
                managed_changed,
                agent_name_changed,
                unchanged_change,
                managed_launch_pending,
                suppress_acquisition_completion,
            )
        };
        if mutation.session_ref_changed || managed_changed || agent_name_changed {
            self.mark_session_dirty();
        }
        let agent_released = mutation.agent_released;
        let change = mutation.effective_state_change.or(unchanged_change)?;
        let suppress_completion = force_suppress_completion
            || (change.state == AgentState::Idle
                && (managed_launch_pending || suppress_acquisition_completion));
        if change.previous_state != change.state {
            self.next_agent_state_change_seq += 1;
            if let Some(terminal) = self.terminals.get_mut(&terminal_id) {
                terminal.last_agent_state_change_seq = Some(self.next_agent_state_change_seq);
            }
        }
        let seen = self.apply_pane_state_change(ws_idx, pane_id, &change, suppress_completion)?;
        let update = PaneStateUpdate {
            pane_id,
            ws_idx,
            previous_agent_label: change.previous_agent_label.clone(),
            previous_known_agent: change.previous_known_agent,
            previous_state: change.previous_state,
            previous_seen,
            previous_presentation: change.previous_presentation.clone(),
            agent_label: if agent_released {
                change.previous_agent_label.clone()
            } else {
                change.agent_label.clone()
            },
            known_agent: if agent_released {
                change.previous_known_agent
            } else {
                change.known_agent
            },
            state: change.state,
            seen,
            presentation: change.presentation.clone(),
            agent_name_changed,
            agent_released,
            agent_release_status: agent_released.then(|| pane_agent_status(change.state, seen)),
            suppress_completion,
        };
        Some(update)
    }

    pub(crate) fn next_managed_agent_deadline(&self) -> Option<Instant> {
        self.terminals
            .values()
            .filter_map(crate::terminal::TerminalState::next_managed_agent_deadline)
            .min()
    }

    pub(crate) fn publish_pane_process_exit_if_agent(
        &mut self,
        pane_id: PaneId,
        suppress_completion: bool,
    ) -> Option<PaneStateUpdate> {
        let observed_at = std::time::Instant::now();
        let update = self.update_terminal_state_with_completion_policy(
            pane_id,
            suppress_completion,
            |terminal| {
                let agent = terminal
                    .effective_known_agent()
                    .or(terminal.detected_agent)?;
                Some(terminal.set_detected_state_with_screen_signals_at(
                    Some(agent),
                    AgentState::Idle,
                    false,
                    true,
                    observed_at,
                ))
            },
        )?;
        update.agent_released.then_some(update)
    }

    fn apply_pane_state_change(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        change: &EffectiveStateChange,
        suppress_completion: bool,
    ) -> Option<bool> {
        let is_active_tab = self.pane_is_in_active_tab(ws_idx, pane_id);
        let suppress_active_tab_notifications =
            active_tab_suppresses_notifications(is_active_tab, self.outer_terminal_focus);
        let pane = self.workspaces[ws_idx]
            .tabs
            .iter_mut()
            .find_map(|tab| tab.panes.get_mut(&pane_id))?;

        if change.state != AgentState::Idle {
            pane.seen = true;
        } else if !suppress_completion && is_completion_transition(change) {
            pane.seen = suppress_active_tab_notifications;
        }
        let seen = pane.seen;

        if !suppress_completion {
            if let Some(delivery) =
                self.record_or_deliver_agent_notification(ws_idx, pane_id, change)
            {
                self.apply_agent_notification_delivery(&delivery);
            }
        }

        Some(seen)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/events_test.rs"]
mod tests;
