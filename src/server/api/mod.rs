pub(crate) mod socket;
pub(crate) mod streams;

pub use socket::ServerHandle;
pub(crate) use socket::{api_method_name, start_server_with_stop_control};
pub use streams::event_hub::EventHub;

use tokio::sync::mpsc;

use crate::protocol::api::schema::{Method, Request};

pub(crate) fn request_changes_ui(request: &Request) -> bool {
    matches!(
        &request.method,
        Method::ServerReloadConfig(_)
            | Method::NotificationShow(_)
            | Method::WorkspaceCreate(_)
            | Method::WorkspaceFocus(_)
            | Method::WorkspaceRename(_)
            | Method::WorkspaceMove(_)
            | Method::WorkspaceClose(_)
            | Method::TabCreate(_)
            | Method::TabFocus(_)
            | Method::TabRename(_)
            | Method::TabClose(_)
            | Method::LayoutSetSplitRatio(_)
            | Method::AgentRename(_)
            | Method::AgentFocus(_)
            | Method::AgentStart(_)
            | Method::AgentPrompt(_)
            | Method::AgentPromptIfIdle(_)
            | Method::AgentPromptIfUnbound(_)
            | Method::AgentDialogChoose(_)
            | Method::AgentDialogAnswer(_)
            | Method::AgentSendKeys(_)
            | Method::PaneSplit(_)
            | Method::PaneSwap(_)
            | Method::PaneZoom(_)
            | Method::PaneFocusDirection(_)
            | Method::PaneResize(_)
            | Method::PaneScroll(_)
            | Method::PaneFocus(_)
            | Method::PaneInputSet(_)
            | Method::PaneRename(_)
            | Method::PaneReportAgentSession(_)
            | Method::PaneClose(_)
            | Method::PaneCloseIfIdentity(_)
    )
}

pub struct ApiRequestMessage {
    pub request: Request,
    pub respond_to: std::sync::mpsc::Sender<String>,
}

pub type ApiRequestSender = mpsc::UnboundedSender<ApiRequestMessage>;

use std::time::{Duration, Instant};

mod agents;
mod env;
mod errors;
pub(crate) mod input_encoding;
mod layouts;
mod panes;
mod session;
mod tabs;
mod workspaces;

use self::input_encoding::pane_agent_status;
use crate::events::AppEvent;
use crate::server::app::{App, Mode, OverlayPaneState, ToastKind};

const API_NOTIFICATION_RATE_LIMIT: Duration = Duration::from_secs(1);
#[cfg(windows)]
const WINDOWS_POWERSHELL_AGENT_EXIT_RESPAWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeExitAction {
    RespawnShell,
    ClosePane,
}

impl App {
    pub(crate) fn handle_internal_event_with_render_impact(&mut self, ev: AppEvent) -> bool {
        match ev {
            ev @ AppEvent::TerminalBell { .. } => {
                self.handle_internal_event(ev);
                false
            }
            ev => {
                self.handle_internal_event(ev);
                true
            }
        }
    }

    pub(crate) fn handle_internal_event(&mut self, ev: AppEvent) {
        let _ = self.handle_internal_event_with_pane_updates(ev);
    }

    pub(crate) fn handle_internal_event_with_pane_updates(
        &mut self,
        ev: AppEvent,
    ) -> Vec<crate::server::terminals::events::PaneStateUpdate> {
        if matches!(
            &ev,
            AppEvent::TerminalBell { .. } | AppEvent::ClipboardWrite { .. }
        ) {
            return Vec::new();
        }

        if let AppEvent::PaneDied { pane_id, .. } = &ev {
            let previous_toast = self.state.toast.clone();
            if let Some(update) = self
                .state
                .publish_pane_process_exit_if_agent(*pane_id, false)
            {
                self.refresh_new_herdr_toast_context_for_update(&update, &previous_toast);
                self.emit_pane_state_update(&update);
            }
            if self.runtime_exit_action(*pane_id) == RuntimeExitAction::RespawnShell
                && self.respawn_shell_for_launch_pane(*pane_id, true)
            {
                self.overlay_panes.remove(pane_id);
                self.render_dirty.request_generic();
                self.render_notify.notify_one();
                return Vec::new();
            }
        }

        let checkpointed_pane_exit = matches!(
            &ev,
            AppEvent::PaneDied {
                pane_id,
                exit_reason,
            } if exit_reason.requires_session_checkpoint() && self.find_pane(*pane_id).is_some() && !self.overlay_panes.contains_key(pane_id)
        );
        if checkpointed_pane_exit {
            self.checkpoint_session_before_pane_exit();
        }

        let overlay_state = if let AppEvent::PaneDied { pane_id, .. } = &ev {
            self.overlay_panes.remove(pane_id).map(|overlay| {
                let was_overlay_active =
                    self.state
                        .is_active_pane(overlay.ws_idx, overlay.tab_idx, *pane_id);
                let tab_before_exit = self
                    .state
                    .workspaces
                    .get(overlay.ws_idx)
                    .and_then(|ws| ws.tabs.get(overlay.tab_idx));
                let was_overlay_focused_in_tab =
                    tab_before_exit.is_some_and(|tab| tab.layout.focused() == *pane_id);
                let tab_zoomed_before_exit = tab_before_exit.map(|tab| tab.zoomed);
                (
                    overlay,
                    was_overlay_active,
                    was_overlay_focused_in_tab,
                    tab_zoomed_before_exit,
                )
            })
        } else {
            None
        };

        if let AppEvent::PaneDied { pane_id, .. } = &ev {
            if let Some((ws_idx, _)) = self.find_pane(*pane_id) {
                if let Some(public_pane_id) = self.public_pane_id(ws_idx, *pane_id) {
                    self.emit_event(crate::protocol::api::schema::EventEnvelope {
                        event: crate::protocol::api::schema::EventKind::PaneExited,
                        data: crate::protocol::api::schema::EventData::PaneExited {
                            pane_id: public_pane_id,
                            workspace_id: self.public_workspace_id(ws_idx),
                        },
                    });
                }
            }
        }
        let pane_exit_layout_target = if let AppEvent::PaneDied { pane_id, .. } = &ev {
            self.find_pane(*pane_id).and_then(|(ws_idx, _)| {
                self.layout_update_target_after_pane_removal(ws_idx, *pane_id)
            })
        } else {
            None
        };

        let terminal_cwd_reported = matches!(ev, AppEvent::TerminalCwdReported { .. });
        let previous_toast = self.state.toast.clone();
        let pane_updates = self.state.handle_app_event(ev);
        if checkpointed_pane_exit {
            self.finish_checkpointed_pane_exit();
        }
        if terminal_cwd_reported {
            self.state
                .refresh_workspace_auto_labels(&self.terminal_runtimes);
            self.render_dirty.request_generic();
            self.render_notify.notify_one();
        }
        for update in &pane_updates {
            self.refresh_new_herdr_toast_context_for_update(update, &previous_toast);
            self.emit_pane_state_update(update);
        }
        if let Some((
            overlay,
            was_overlay_active,
            was_overlay_focused_in_tab,
            tab_zoomed_before_exit,
        )) = overlay_state
        {
            self.restore_overlay_after_exit(
                overlay,
                was_overlay_active,
                was_overlay_focused_in_tab,
                tab_zoomed_before_exit,
            );
        }
        if let Some((ws_idx, tab_idx)) = pane_exit_layout_target {
            self.emit_layout_updated_event(ws_idx, tab_idx);
        }

        self.sync_toast_deadline(previous_toast);
        self.shutdown_detached_terminal_runtimes();
        pane_updates
    }

    pub(crate) fn refresh_new_herdr_toast_context_for_update(
        &mut self,
        update: &crate::server::terminals::events::PaneStateUpdate,
        previous_toast: &Option<crate::server::app_state::ToastNotification>,
    ) {
        if !matches!(
            self.state.toast_config.delivery,
            crate::config::ToastDelivery::Herdr
        ) || self.state.toast == *previous_toast
        {
            return;
        }

        let Some(target) = self
            .state
            .toast
            .as_ref()
            .and_then(|toast| toast.target.as_ref())
        else {
            return;
        };
        if target.pane_id != update.pane_id {
            return;
        }
        let Some(ws) = self.state.workspaces.get(update.ws_idx) else {
            return;
        };
        if ws.id != target.workspace_id {
            return;
        }

        let workspace_label = ws.display_name_from(&self.state.terminals, &self.terminal_runtimes);
        let context = crate::server::terminals::events::notification_context(
            ws,
            &workspace_label,
            update.ws_idx,
            update.pane_id,
        );
        if let Some(toast) = self.state.toast.as_mut() {
            toast.context = context;
        }
    }

    fn restore_overlay_after_exit(
        &mut self,
        overlay: OverlayPaneState,
        was_overlay_active: bool,
        was_overlay_focused_in_tab: bool,
        tab_zoomed_before_exit: Option<bool>,
    ) {
        for temp_file in &overlay.temp_files {
            let _ = std::fs::remove_file(temp_file);
        }

        let Some(ws) = self.state.workspaces.get_mut(overlay.ws_idx) else {
            return;
        };
        if overlay.tab_idx >= ws.tabs.len() {
            return;
        }

        if !was_overlay_focused_in_tab {
            if let Some(tab_zoomed_before_exit) = tab_zoomed_before_exit {
                ws.tabs[overlay.tab_idx].zoomed = tab_zoomed_before_exit;
            }
            return;
        }

        if was_overlay_active {
            ws.active_tab = overlay.tab_idx;
        }
        let tab = &mut ws.tabs[overlay.tab_idx];
        if tab.panes.contains_key(&overlay.previous_focus) {
            tab.layout.focus_pane(overlay.previous_focus);
        }
        tab.zoomed = overlay.previous_zoomed;

        if was_overlay_active && self.state.active == Some(overlay.ws_idx) {
            self.state.mode = Mode::Terminal;
        }
    }

    fn runtime_exit_action(
        &self,
        pane_id: crate::server::workspaces::layout::PaneId,
    ) -> RuntimeExitAction {
        let Some((_, pane_state)) = self.find_pane(pane_id) else {
            return RuntimeExitAction::ClosePane;
        };
        let Some(terminal) = self.state.terminals.get(&pane_state.attached_terminal_id) else {
            return RuntimeExitAction::ClosePane;
        };

        if terminal.respawn_shell_on_exit || self.should_respawn_shell_after_agent_exit(terminal) {
            RuntimeExitAction::RespawnShell
        } else {
            RuntimeExitAction::ClosePane
        }
    }

    fn should_respawn_shell_after_agent_exit(
        &self,
        terminal: &crate::terminal::TerminalState,
    ) -> bool {
        #[cfg(not(windows))]
        {
            let _ = terminal;
            false
        }

        #[cfg(windows)]
        {
            if !terminal.agent_process_exited_within(
                Instant::now(),
                WINDOWS_POWERSHELL_AGENT_EXIT_RESPAWN_GRACE,
            ) {
                return false;
            }

            crate::pane::uses_windows_powershell_pane_shell(crate::pane::PaneShellConfig::new(
                &self.state.default_shell,
                self.state.shell_mode,
            ))
        }
    }

    fn respawn_shell_for_launch_pane(
        &mut self,
        pane_id: crate::server::workspaces::layout::PaneId,
        focus_pane: bool,
    ) -> bool {
        let Some((ws_idx, pane_state)) = self.find_pane(pane_id) else {
            return false;
        };
        let terminal_id = pane_state.attached_terminal_id.clone();
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return false;
        };

        let cwd = terminal.cwd.clone();
        let (rows, cols) = self
            .terminal_runtimes
            .get(&terminal_id)
            .map(|runtime| runtime.current_size())
            .unwrap_or_else(|| self.state.estimate_pane_size());
        let Some(launch_env) = self.pane_launch_env(ws_idx, pane_id, Vec::new()) else {
            return false;
        };
        let runtime = match crate::terminal::TerminalRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.state.host_terminal_appearance,
            crate::pane::PaneShellConfig::new(&self.state.default_shell, self.state.shell_mode),
            &launch_env,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(
                    pane = pane_id.raw(),
                    terminal = %terminal_id,
                    err = %err,
                    "failed to respawn shell after launch command exited"
                );
                return false;
            }
        };

        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.clear_agent_runtime_identity_after_respawn();
        }
        if focus_pane {
            self.state.focus_pane_in_workspace(ws_idx, pane_id);
        }
        self.schedule_session_save();
        true
    }

    pub(crate) fn emit_pane_state_update(
        &mut self,
        update: &crate::server::terminals::events::PaneStateUpdate,
    ) {
        let Some(pane_id) = self.public_pane_id(update.ws_idx, update.pane_id) else {
            return;
        };
        let workspace_id = self.public_workspace_id(update.ws_idx);

        if update.agent_name_changed {
            self.emit_pane_updated(update.ws_idx, update.pane_id);
        }

        if update.previous_agent_label != update.agent_label || update.agent_released {
            self.emit_event(crate::protocol::api::schema::EventEnvelope {
                event: crate::protocol::api::schema::EventKind::PaneAgentDetected,
                data: crate::protocol::api::schema::EventData::PaneAgentDetected {
                    pane_id: pane_id.clone(),
                    workspace_id: workspace_id.clone(),
                    agent: update.agent_label.clone(),
                    released: update.agent_released,
                    final_status: update.agent_release_status,
                },
            });
        }

        let previous_agent_status = pane_agent_status(update.previous_state, update.previous_seen);
        let agent_status = self
            .state
            .workspaces
            .get(update.ws_idx)
            .and_then(|ws| ws.pane_state(update.pane_id))
            .map(|pane| pane_agent_status(update.state, pane.seen))
            .unwrap_or_else(|| pane_agent_status(update.state, update.seen));

        if previous_agent_status != agent_status {
            self.emit_event(crate::protocol::api::schema::EventEnvelope {
                event: crate::protocol::api::schema::EventKind::PaneAgentStatusChanged,
                data: crate::protocol::api::schema::EventData::PaneAgentStatusChanged {
                    pane_id,
                    workspace_id,
                    agent_status,
                    agent: update.agent_label.clone(),
                    title: None,
                    display_agent: None,
                    state_labels: std::collections::HashMap::new(),
                },
            });
        }
    }

    pub(crate) fn sync_toast_deadline(
        &mut self,
        previous_toast: Option<crate::server::app_state::ToastNotification>,
    ) {
        if self.state.toast != previous_toast {
            self.toast_deadline = self.state.toast.as_ref().map(|toast| {
                let duration = match toast.kind {
                    ToastKind::NeedsAttention => Duration::from_secs(8),
                    ToastKind::Finished => Duration::from_secs(5),
                    ToastKind::UpdateInstalled => Duration::from_secs(3),
                };
                Instant::now() + duration
            });
        }
    }

    pub(crate) fn refresh_agent_notification_delivery_contexts(
        &mut self,
        deliveries: &mut [crate::server::app_state::AgentNotificationDelivery],
    ) {
        for delivery in deliveries {
            let Some(ws_idx) = self
                .state
                .workspaces
                .iter()
                .position(|ws| ws.id == delivery.workspace_id)
            else {
                continue;
            };
            let ws = &self.state.workspaces[ws_idx];
            let workspace_label =
                ws.display_name_from(&self.state.terminals, &self.terminal_runtimes);
            let context = crate::server::terminals::events::notification_context(
                ws,
                &workspace_label,
                ws_idx,
                delivery.pane_id,
            );
            if let Some(toast) = delivery.toast.as_mut() {
                toast.context = context.clone();
            }
            if let Some(toast) = delivery.client_notification.as_mut() {
                toast.context = context.clone();
            }
            if let Some(toast) = self.state.toast.as_mut() {
                if toast.target.as_ref().is_some_and(|target| {
                    target.workspace_id == delivery.workspace_id
                        && target.pane_id == delivery.pane_id
                }) {
                    toast.context = context;
                }
            }
        }
    }

    pub(super) fn emit_event(&mut self, event: crate::protocol::api::schema::EventEnvelope) {
        self.event_hub.push(event);
    }

    pub(crate) fn emit_pane_updated(
        &mut self,
        ws_idx: usize,
        pane_id: crate::server::workspaces::layout::PaneId,
    ) {
        if let Some(pane) = self.pane_info(ws_idx, pane_id) {
            self.emit_event(crate::protocol::api::schema::EventEnvelope {
                event: crate::protocol::api::schema::EventKind::PaneUpdated,
                data: crate::protocol::api::schema::EventData::PaneUpdated { pane },
            });
        }
    }

    pub(crate) fn sync_focus_events(&mut self) {
        self.sync_focus_events_with_outer_event(None);
    }

    pub(crate) fn accept_current_focus_without_events(&mut self) {
        self.last_focus = self.state.active.and_then(|idx| {
            self.state
                .workspaces
                .get(idx)
                .and_then(|workspace| workspace.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
    }

    pub(crate) fn accept_current_focus_with_api_events(&mut self) {
        let current_focus = self.state.active.and_then(|idx| {
            self.state
                .workspaces
                .get(idx)
                .and_then(|workspace| workspace.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        if current_focus == self.last_focus {
            return;
        }
        self.last_focus = current_focus;
        if let Some((ws_idx, pane_id)) = current_focus {
            self.emit_focus_api_events(ws_idx, pane_id);
        }
    }

    fn emit_focus_api_events(
        &mut self,
        ws_idx: usize,
        pane_id: crate::server::workspaces::layout::PaneId,
    ) {
        self.emit_event(crate::protocol::api::schema::EventEnvelope {
            event: crate::protocol::api::schema::EventKind::WorkspaceFocused,
            data: crate::protocol::api::schema::EventData::WorkspaceFocused {
                workspace_id: self.public_workspace_id(ws_idx),
            },
        });
        if let Some(tab_id) = self.public_tab_id(ws_idx, self.state.workspaces[ws_idx].active_tab) {
            self.emit_event(crate::protocol::api::schema::EventEnvelope {
                event: crate::protocol::api::schema::EventKind::TabFocused,
                data: crate::protocol::api::schema::EventData::TabFocused {
                    tab_id,
                    workspace_id: self.public_workspace_id(ws_idx),
                },
            });
        }
        if let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) {
            self.emit_event(crate::protocol::api::schema::EventEnvelope {
                event: crate::protocol::api::schema::EventKind::PaneFocused,
                data: crate::protocol::api::schema::EventData::PaneFocused {
                    pane_id: public_pane_id,
                    workspace_id: self.public_workspace_id(ws_idx),
                },
            });
        }
    }

    fn sync_focus_events_with_outer_event(
        &mut self,
        outer_event: Option<crate::ghostty::FocusEvent>,
    ) {
        let current_focus = self.state.active.and_then(|idx| {
            self.state
                .workspaces
                .get(idx)
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        if current_focus == self.last_focus {
            if let (Some((ws_idx, pane_id)), Some(event)) = (current_focus, outer_event) {
                self.send_pane_focus_event(ws_idx, pane_id, event);
            }
            return;
        }

        if let Some((ws_idx, pane_id)) = self.last_focus {
            self.send_pane_focus_event(ws_idx, pane_id, crate::ghostty::FocusEvent::Lost);
        }
        if let Some((ws_idx, pane_id)) = current_focus {
            let event = outer_event.unwrap_or_else(|| {
                if self.state.outer_terminal_focus == Some(false) {
                    crate::ghostty::FocusEvent::Lost
                } else {
                    crate::ghostty::FocusEvent::Gained
                }
            });
            self.send_pane_focus_event(ws_idx, pane_id, event);
            self.emit_focus_api_events(ws_idx, pane_id);
        }

        self.last_focus = current_focus;
    }

    pub(crate) fn send_pane_focus_event(
        &self,
        ws_idx: usize,
        pane_id: crate::server::workspaces::layout::PaneId,
        event: crate::ghostty::FocusEvent,
    ) {
        let Some(runtime) = self.state.workspaces.get(ws_idx).and_then(|_| {
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        }) else {
            return;
        };
        runtime.try_send_focus_event(event);
    }

    #[cfg(test)]
    pub(crate) fn handle_api_request(
        &mut self,
        request: crate::protocol::api::schema::Request,
    ) -> String {
        self.drain_all_internal_events();
        self.handle_api_request_after_internal_events_drained(request)
    }

    pub(crate) fn handle_api_request_after_internal_events_drained(
        &mut self,
        request: crate::protocol::api::schema::Request,
    ) -> String {
        self.sync_pending_terminal_titles();
        use crate::protocol::api::schema::{Method, ResponseResult, SuccessResponse};

        let response = match request.method {
            Method::ServerStop(_) => {
                self.state.should_quit = true;
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::Ok {},
                }
            }
            Method::ServerReloadConfig(_) => {
                let report = self.reload_config();
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::ConfigReload {
                        status: report.status,
                        diagnostics: report.diagnostics,
                    },
                }
            }
            Method::NotificationShow(params) => {
                return self.handle_notification_show(request.id, params);
            }
            Method::ClientWindowTitleSet(_) | Method::ClientWindowTitleClear(_) => {
                return errors::encode_success(
                    request.id,
                    ResponseResult::ClientWindowTitle {
                        changed: false,
                        reason: crate::protocol::api::schema::ClientWindowTitleReason::NoForegroundClient,
                    },
                );
            }
            Method::SessionSnapshot(_) => return self.handle_session_snapshot(request.id),
            Method::WorkspaceList(_) => return self.handle_workspace_list(request.id),
            Method::WorkspaceGet(target) => return self.handle_workspace_get(request.id, target),
            Method::WorkspaceCreate(params) => {
                return self.handle_workspace_create(request.id, params);
            }
            Method::WorkspaceFocus(target) => {
                return self.handle_workspace_focus(request.id, target)
            }
            Method::WorkspaceRename(params) => {
                return self.handle_workspace_rename(request.id, params);
            }
            Method::WorkspaceMove(params) => {
                return self.handle_workspace_move(request.id, params);
            }
            Method::WorkspaceClose(target) => {
                return self.handle_workspace_close(request.id, target)
            }
            Method::TabList(params) => return self.handle_tab_list(request.id, params),
            Method::TabGet(target) => return self.handle_tab_get(request.id, target),
            Method::TabCreate(params) => return self.handle_tab_create(request.id, params),
            Method::TabFocus(target) => return self.handle_tab_focus(request.id, target),
            Method::TabRename(params) => return self.handle_tab_rename(request.id, params),
            Method::TabClose(target) => return self.handle_tab_close(request.id, target),
            Method::AgentList(_) => return self.handle_agent_list(request.id),
            Method::AgentGet(target) => return self.handle_agent_get(request.id, target),
            Method::AgentFocus(target) => return self.handle_agent_focus(request.id, target),
            Method::AgentRename(params) => return self.handle_agent_rename(request.id, params),
            Method::AgentStart(params) => return self.handle_agent_start(request.id, params),
            Method::AgentPrompt(_)
            | Method::AgentPromptIfIdle(_)
            | Method::AgentPromptIfUnbound(_) => {
                return errors::encode_error(
                    request.id,
                    "invalid_request",
                    "agent.prompt is handled asynchronously by the app runtime",
                );
            }
            Method::AgentWait(_) => {
                return errors::encode_error(
                    request.id,
                    "invalid_request",
                    "agent.wait is handled by the api server",
                );
            }
            Method::AgentRead(params) => return self.handle_agent_read(request.id, params),
            Method::AgentDialogObserve(target) => {
                return self.handle_agent_dialog_observe(request.id, target)
            }
            Method::AgentDialogChoose(params) => {
                return self.handle_agent_dialog_choose(request.id, params)
            }
            Method::AgentDialogAnswer(params) => {
                return self.handle_agent_dialog_answer(request.id, params)
            }
            Method::AgentSendKeys(params) => {
                return self.handle_agent_send_keys(request.id, params)
            }
            Method::PaneSplit(params) => return self.handle_pane_split(request.id, params),
            Method::PaneSwap(params) => return self.handle_pane_swap(request.id, params),
            Method::PaneZoom(params) => return self.handle_pane_zoom(request.id, params),
            Method::PaneLayout(params) => return self.handle_pane_layout(request.id, params),
            Method::LayoutSetSplitRatio(params) => {
                return self.handle_layout_set_split_ratio(request.id, params);
            }
            Method::PaneFocusDirection(params) => {
                return self.handle_pane_focus_direction(request.id, params);
            }
            Method::PaneResize(params) => return self.handle_pane_resize(request.id, params),
            Method::PaneScroll(params) => return self.handle_pane_scroll(request.id, params),
            Method::PaneSelectionRead(params) => {
                return self.handle_pane_selection_read(request.id, params);
            }
            Method::PaneCopyMotion(params) => {
                return self.handle_pane_copy_motion(request.id, params);
            }
            Method::PaneCopySearch(params) => {
                return self.handle_pane_copy_search(request.id, params);
            }
            Method::PaneList(params) => return self.handle_pane_list(request.id, params),
            Method::PaneCurrent(params) => return self.handle_pane_current(request.id, params),
            Method::PaneGet(target) => return self.handle_pane_get(request.id, target),
            Method::PaneFocus(target) => return self.handle_pane_focus(request.id, target),
            Method::PaneInputSet(params) => return self.handle_pane_input_set(request.id, params),
            Method::PaneLinkActivate(params) => {
                return self.handle_pane_link_activate(request.id, params);
            }
            Method::PaneRename(params) => return self.handle_pane_rename(request.id, params),
            Method::PaneRead(params) => return self.handle_pane_read(request.id, params),
            Method::PaneReportAgentSession(params) => {
                return self.handle_pane_report_agent_session(request.id, params);
            }
            Method::PaneSendText(params) => return self.handle_pane_send_text(request.id, params),
            Method::PaneSendInput(params) => {
                return self.handle_pane_send_input(request.id, params)
            }
            Method::PaneClose(target) => return self.handle_pane_close(request.id, target),
            Method::PaneCloseIfIdentity(params) => {
                return self.handle_pane_close_if_identity(request.id, params)
            }
            Method::PaneSendKeys(params) => return self.handle_pane_send_keys(request.id, params),
            _ => {
                return errors::encode_error(
                    request.id,
                    "not_implemented",
                    "method not implemented yet",
                );
            }
        };

        serde_json::to_string(&response).unwrap()
    }

    fn handle_notification_show(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::NotificationShowParams,
    ) -> String {
        use crate::protocol::api::schema::{NotificationShowReason, ResponseResult};

        let Some(title) = sanitized_notification_text(&params.title, 80) else {
            return errors::encode_error(id, "invalid_params", "notification title is empty");
        };
        let body = params
            .body
            .as_deref()
            .and_then(|body| sanitized_notification_text(body, 240));

        let reason = match self.state.toast_config.delivery {
            crate::config::ToastDelivery::Off => NotificationShowReason::Disabled,
            crate::config::ToastDelivery::Herdr => {
                if self.state.toast.is_some() {
                    NotificationShowReason::Busy
                } else if self.api_notification_rate_limited(Instant::now()) {
                    NotificationShowReason::RateLimited
                } else {
                    let previous_toast = self.state.toast.clone();
                    self.mark_api_notification_shown(Instant::now());
                    self.state.toast = Some(crate::server::app_state::ToastNotification {
                        kind: ToastKind::UpdateInstalled,
                        title,
                        context: body.unwrap_or_default(),
                        position: params.position,
                        target: None,
                    });
                    self.sync_toast_deadline(previous_toast);
                    NotificationShowReason::Shown
                }
            }
            crate::config::ToastDelivery::Terminal | crate::config::ToastDelivery::System => {
                NotificationShowReason::NoForegroundClient
            }
        };

        errors::encode_success(
            id,
            ResponseResult::NotificationShow {
                shown: matches!(reason, NotificationShowReason::Shown),
                reason,
            },
        )
    }

    pub(crate) fn api_notification_rate_limited(&self, now: Instant) -> bool {
        self.last_api_notification_at
            .is_some_and(|last| now.duration_since(last) < API_NOTIFICATION_RATE_LIMIT)
    }

    pub(crate) fn mark_api_notification_shown(&mut self, now: Instant) {
        self.last_api_notification_at = Some(now);
    }
}

fn sanitized_notification_text(value: &str, max_chars: usize) -> Option<String> {
    let mut sanitized = String::new();
    let mut previous_space = false;
    for ch in value.chars() {
        let replacement = if ch == '\n' || ch == '\r' || ch == '\t' {
            Some(' ')
        } else if ch.is_control() {
            None
        } else {
            Some(ch)
        };
        let Some(ch) = replacement else {
            continue;
        };
        if ch.is_whitespace() {
            if previous_space {
                continue;
            }
            previous_space = true;
            sanitized.push(' ');
        } else {
            previous_space = false;
            sanitized.push(ch);
        }
        if sanitized.chars().count() >= max_chars {
            break;
        }
    }
    let sanitized = sanitized.trim().to_string();
    (!sanitized.is_empty()).then_some(sanitized)
}

#[cfg(test)]
pub(super) mod test_support {
    pub(crate) fn exiting_test_command() -> &'static str {
        #[cfg(windows)]
        {
            "C:\\Windows\\System32\\whoami.exe"
        }
        #[cfg(not(windows))]
        {
            "/usr/bin/true"
        }
    }

    pub(crate) fn shutdown_test_runtimes(app: &mut crate::server::app::App) {
        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Agent, AgentState};

    #[cfg(unix)]
    fn init_repo(path: &std::path::Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git init failed for {}", path.display());
    }

    fn app_with_overlay(
        workspace: crate::server::workspaces::Workspace,
        overlay_pane: crate::server::workspaces::layout::PaneId,
        previous_focus: crate::server::workspaces::layout::PaneId,
        previous_zoomed: bool,
    ) -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.mode = Mode::Terminal;
        app.overlay_panes.insert(
            overlay_pane,
            OverlayPaneState {
                ws_idx: 0,
                tab_idx: 0,
                previous_focus,
                previous_zoomed,
                temp_files: Vec::new(),
            },
        );
        app
    }

    #[test]
    fn client_window_title_api_reports_no_foreground_client_in_app_mode() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );

        let set = app.handle_api_request(crate::protocol::api::schema::Request {
            id: "title_set".into(),
            method: crate::protocol::api::schema::Method::ClientWindowTitleSet(
                crate::protocol::api::schema::ClientWindowTitleSetParams {
                    title: "plugin review".into(),
                },
            ),
        });
        let set: serde_json::Value = serde_json::from_str(&set).unwrap();
        assert_eq!(set["result"]["type"], "client_window_title");
        assert_eq!(set["result"]["changed"], false);
        assert_eq!(set["result"]["reason"], "no_foreground_client");

        let clear = app.handle_api_request(crate::protocol::api::schema::Request {
            id: "title_clear".into(),
            method: crate::protocol::api::schema::Method::ClientWindowTitleClear(
                crate::protocol::api::schema::EmptyParams::default(),
            ),
        });
        let clear: serde_json::Value = serde_json::from_str(&clear).unwrap();
        assert_eq!(clear["result"]["type"], "client_window_title");
        assert_eq!(clear["result"]["reason"], "no_foreground_client");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn herdr_toast_context_uses_live_root_runtime_cwd_label() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );

        let mut workspace = crate::server::workspaces::Workspace::test_new("stale");
        workspace.custom_name = None;
        let root = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(root).cloned().unwrap();
        let temp_root = std::env::temp_dir().join(format!(
            "herdr-toast-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stale_cwd = temp_root.join("__herdr_original__");
        let live_cwd = temp_root.join("__herdr_projects__");
        std::fs::create_dir_all(&stale_cwd).unwrap();
        std::fs::create_dir_all(&live_cwd).unwrap();
        init_repo(&stale_cwd);
        init_repo(&live_cwd);

        workspace.identity_cwd = stale_cwd.clone();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = stale_cwd;
        app.state.active = None;
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
        app.state.toast_config.delay_seconds = 0;

        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            root,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            &crate::pane::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(crate::render_signal::RenderSignal::new()),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        app.terminal_runtimes.insert(terminal_id, runtime);

        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: root,
            agent: Some(Agent::Codex),
            state: AgentState::Working,
            visible_blocker: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });
        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: root,
            agent: Some(Agent::Codex),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });

        assert_eq!(
            app.state.toast.as_ref().map(|toast| toast.context.as_str()),
            Some("__herdr_projects__ · 1")
        );

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delayed_herdr_toast_context_uses_live_root_runtime_cwd_label() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );

        let mut workspace = crate::server::workspaces::Workspace::test_new("stale");
        workspace.custom_name = None;
        let root = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(root).cloned().unwrap();
        let temp_root = std::env::temp_dir().join(format!(
            "herdr-delayed-toast-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stale_cwd = temp_root.join("__herdr_original__");
        let live_cwd = temp_root.join("__herdr_projects__");
        std::fs::create_dir_all(&stale_cwd).unwrap();
        std::fs::create_dir_all(&live_cwd).unwrap();
        init_repo(&stale_cwd);
        init_repo(&live_cwd);

        workspace.identity_cwd = stale_cwd.clone();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = stale_cwd;
        app.state.active = None;
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
        app.state.toast_config.delay_seconds = 1;

        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            root,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            &crate::pane::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(crate::render_signal::RenderSignal::new()),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        app.terminal_runtimes.insert(terminal_id, runtime);

        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: root,
            agent: Some(Agent::Codex),
            state: AgentState::Working,
            visible_blocker: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });
        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: root,
            agent: Some(Agent::Codex),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });

        let notification_deadline = app
            .state
            .next_pending_agent_notification_deadline()
            .expect("pending notification deadline");
        let mut deliveries = app
            .state
            .drain_due_agent_notifications(notification_deadline);
        app.refresh_agent_notification_delivery_contexts(&mut deliveries);
        assert_eq!(
            deliveries
                .first()
                .and_then(|delivery| delivery.toast.as_ref())
                .map(|toast| toast.context.as_str()),
            Some("__herdr_projects__ · 1")
        );

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn overlay_exit_preserves_focus_changed_before_exit() {
        let mut workspace = crate::server::workspaces::Workspace::test_new("overlay");
        let previous_focus = workspace.tabs[0].root_pane;
        let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].zoomed = true;
        let new_tab = workspace.test_add_tab(Some("new"));
        workspace.switch_tab(new_tab);
        let mut app = app_with_overlay(workspace, overlay_pane, previous_focus, true);

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: overlay_pane,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        let overlay_tab = &app.state.workspaces[0].tabs[0];
        assert_eq!(app.state.workspaces[0].active_tab, new_tab);
        assert_eq!(overlay_tab.layout.focused(), previous_focus);
        assert!(overlay_tab.zoomed);
        assert!(app.overlay_panes.is_empty());
    }

    #[test]
    fn pane_exit_emits_layout_updated_when_tab_survives() {
        let event_hub = crate::server::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            event_hub.clone(),
        );
        let mut workspace = crate::server::workspaces::Workspace::test_new("pane-exit-layout");
        let dead_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let tab_id = app.public_tab_id(0, 0).unwrap();

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: dead_pane,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        let events = event_hub.events_after(0);
        let pane_exited = events
            .iter()
            .position(|(_, event)| {
                event.event == crate::protocol::api::schema::EventKind::PaneExited
            })
            .expect("pane.exited should be emitted");
        let layout_updated = events
            .iter()
            .position(|(_, event)| {
                event.event == crate::protocol::api::schema::EventKind::LayoutUpdated
            })
            .expect("layout.updated should be emitted");
        assert!(pane_exited < layout_updated);
        assert!(matches!(
            &events[layout_updated].1.data,
            crate::protocol::api::schema::EventData::LayoutUpdated { layout }
                if layout.tab_id == tab_id && layout.panes.len() == 1
        ));
    }

    #[test]
    fn idle_agent_exit_emits_release_event_without_a_state_change() {
        for agent_name in [None, Some("reviewer")] {
            let event_hub = crate::server::api::EventHub::default();
            let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut app = App::new(
                &crate::config::Config::default(),
                crate::server::app::AppPolicy::TEST,
                None,
                api_rx,
                event_hub.clone(),
            );
            let workspace = crate::server::workspaces::Workspace::test_new("idle-agent-exit");
            let pane_id = workspace.tabs[0].root_pane;
            let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
            app.state.workspaces = vec![workspace];
            app.state.ensure_test_terminals();
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
            if let Some(agent_name) = agent_name {
                terminal.set_agent_name(agent_name.into());
            }

            app.handle_internal_event(AppEvent::StateChanged {
                pane_id,
                agent: Some(Agent::Pi),
                state: AgentState::Idle,
                visible_blocker: false,
                process_exited: true,
                observed_at: std::time::Instant::now(),
            });

            assert!(app.state.terminals[&terminal_id].agent_name.is_none());
            assert!(event_hub.events_after(0).iter().any(|(_, event)| matches!(
                &event.data,
                crate::protocol::api::schema::EventData::PaneAgentDetected {
                    released: true,
                    final_status: Some(crate::protocol::api::schema::AgentStatus::Idle),
                    ..
                }
            )));
        }
    }

    #[test]
    fn overlay_exit_layout_updated_uses_restored_zoom_state() {
        let event_hub = crate::server::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            event_hub.clone(),
        );
        let mut workspace = crate::server::workspaces::Workspace::test_new("overlay-layout");
        let previous_focus = workspace.tabs[0].root_pane;
        let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(previous_focus);
        workspace.tabs[0].zoomed = true;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.mode = Mode::Terminal;
        let tab_id = app.public_tab_id(0, 0).unwrap();
        app.overlay_panes.insert(
            overlay_pane,
            OverlayPaneState {
                ws_idx: 0,
                tab_idx: 0,
                previous_focus,
                previous_zoomed: false,
                temp_files: Vec::new(),
            },
        );

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: overlay_pane,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        let events = event_hub.events_after(0);
        let layout_updated = events
            .iter()
            .rposition(|(_, event)| {
                event.event == crate::protocol::api::schema::EventKind::LayoutUpdated
            })
            .expect("layout.updated should be emitted");
        assert!(matches!(
            &events[layout_updated].1.data,
            crate::protocol::api::schema::EventData::LayoutUpdated { layout }
                if layout.tab_id == tab_id && layout.zoomed
        ));
    }

    #[test]
    fn overlay_exit_preserves_same_tab_focus_changed_before_exit() {
        let mut workspace = crate::server::workspaces::Workspace::test_new("overlay");
        let previous_focus = workspace.tabs[0].root_pane;
        let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(previous_focus);
        workspace.tabs[0].zoomed = true;
        let mut app = app_with_overlay(workspace, overlay_pane, previous_focus, false);

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: overlay_pane,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        let tab = &app.state.workspaces[0].tabs[0];
        assert_eq!(app.state.workspaces[0].active_tab, 0);
        assert_eq!(tab.layout.focused(), previous_focus);
        assert!(tab.zoomed);
        assert!(app.overlay_panes.is_empty());
    }

    #[test]
    fn overlay_exit_restores_previous_focus_when_overlay_still_focused() {
        let mut workspace = crate::server::workspaces::Workspace::test_new("overlay");
        let previous_focus = workspace.tabs[0].root_pane;
        let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].zoomed = true;
        let mut app = app_with_overlay(workspace, overlay_pane, previous_focus, false);

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: overlay_pane,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        let tab = &app.state.workspaces[0].tabs[0];
        assert_eq!(app.state.workspaces[0].active_tab, 0);
        assert_eq!(tab.layout.focused(), previous_focus);
        assert!(!tab.zoomed);
        assert!(app.overlay_panes.is_empty());
    }

    #[tokio::test]
    async fn pane_died_respawns_shell_and_clears_restored_agent_session() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        let workspace = crate::server::workspaces::Workspace::test_new("restored");
        app.state.default_shell = test_support::exiting_test_command().into();
        let pane_id = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let terminal = app
            .state
            .terminals
            .get_mut(&terminal_id)
            .expect("test terminal should exist");
        terminal.respawn_shell_on_exit = true;
        terminal.set_agent_name("codex".into());
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: crate::agent_resume::AgentSessionRef::id("codex-session")
                .expect("test session id should be valid"),
        });

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        assert!(
            app.find_pane(pane_id).is_some(),
            "respawnable agent pane should stay attached after the agent process exits"
        );
        let terminal = app
            .state
            .terminals
            .get(&terminal_id)
            .expect("terminal should survive respawn");
        assert!(!terminal.respawn_shell_on_exit);
        assert!(terminal.persisted_agent_session.is_none());
        assert!(terminal.agent_name.is_none());

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_exit_after_agent_process_exit_respawns_shell() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        let workspace = crate::server::workspaces::Workspace::test_new("powershell");
        let pane_id = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.default_shell = "powershell.exe".into();
        app.state.shell_mode = crate::config::ShellModeConfig::NonLogin;

        app.handle_internal_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(crate::detect::Agent::OpenCode),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: true,
            observed_at: std::time::Instant::now(),
        });

        assert_eq!(
            app.runtime_exit_action(pane_id),
            RuntimeExitAction::RespawnShell
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_exit_without_recent_agent_process_exit_closes_pane() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        let workspace = crate::server::workspaces::Workspace::test_new("powershell");
        let pane_id = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.default_shell = "powershell.exe".into();
        app.state.shell_mode = crate::config::ShellModeConfig::NonLogin;

        assert_eq!(
            app.runtime_exit_action(pane_id),
            RuntimeExitAction::ClosePane
        );
    }

    #[test]
    fn terminal_delivery_does_not_refresh_existing_targeted_toast() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        let mut workspace = crate::server::workspaces::Workspace::test_new("stale");
        workspace.custom_name = None;
        workspace.identity_cwd = "/__herdr_original__".into();
        let root = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(root).cloned().unwrap();
        let workspace_id = workspace.id.clone();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = "/__herdr_projects__".into();
        app.state.active = None;
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.toast_config.delivery = crate::config::ToastDelivery::Terminal;

        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: root,
            agent: Some(Agent::Codex),
            state: AgentState::Working,
            visible_blocker: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });
        app.state.toast = Some(crate::server::app_state::ToastNotification {
            kind: ToastKind::Finished,
            title: "codex finished".into(),
            context: "__herdr_original__ · 1".into(),
            position: None,
            target: Some(crate::server::app_state::ToastTarget {
                workspace_id,
                pane_id: root,
            }),
        });

        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: root,
            agent: Some(Agent::Codex),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });

        assert_eq!(
            app.state.toast.as_ref().map(|toast| toast.context.as_str()),
            Some("__herdr_original__ · 1")
        );
    }
}
