//! Apply terminal lifecycle events and publish API event envelopes.
use crate::protocol::api::schema::{EventData, EventEnvelope, EventKind};
use crate::server::api::input_encoding::{pane_agent_status, tab_attention_priority};
use crate::server::app::{App, Mode, OverlayPaneState};
use crate::server::terminals::respawn::RuntimeExitAction;
use crate::terminal::events::TerminalEvent;
impl App {
    pub(in crate::server) fn handle_internal_event_with_render_impact(
        &mut self,
        ev: TerminalEvent,
    ) -> bool {
        match ev {
            ev @ TerminalEvent::TerminalBell { .. } => {
                self.handle_internal_event(ev);
                false
            }
            ev => {
                self.handle_internal_event(ev);
                true
            }
        }
    }

    pub(in crate::server) fn handle_internal_event(&mut self, ev: TerminalEvent) {
        let _ = self.handle_internal_event_with_pane_updates(ev);
    }

    pub(in crate::server) fn handle_internal_event_with_pane_updates(
        &mut self,
        ev: TerminalEvent,
    ) -> Vec<crate::server::terminals::events::PaneStateUpdate> {
        if matches!(
            &ev,
            TerminalEvent::TerminalBell { .. } | TerminalEvent::ClipboardWrite { .. }
        ) {
            return Vec::new();
        }

        if let TerminalEvent::PaneDied { pane_id, .. } = &ev {
            let previous_toast = self.state.toast.clone();
            if let Some(update) = self
                .state
                .publish_pane_process_exit_if_agent(*pane_id, false)
            {
                self.refresh_new_bus_toast_context_for_update(&update, &previous_toast);
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
            TerminalEvent::PaneDied {
                pane_id,
                exit_reason,
            } if exit_reason.requires_session_checkpoint() && self.find_pane(*pane_id).is_some() && !self.overlay_panes.contains_key(pane_id)
        );
        if checkpointed_pane_exit {
            self.checkpoint_session_before_pane_exit();
        }

        let overlay_state = self.take_exited_overlay_state(&ev);

        if let TerminalEvent::PaneDied { pane_id, .. } = &ev {
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
        let pane_exit_layout_target = if let TerminalEvent::PaneDied { pane_id, .. } = &ev {
            self.find_pane(*pane_id).and_then(|(ws_idx, _)| {
                self.layout_update_target_after_pane_removal(ws_idx, *pane_id)
            })
        } else {
            None
        };

        let terminal_cwd_reported = matches!(ev, TerminalEvent::TerminalCwdReported { .. });
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
            self.refresh_new_bus_toast_context_for_update(update, &previous_toast);
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

    fn take_exited_overlay_state(
        &mut self,
        ev: &TerminalEvent,
    ) -> Option<(OverlayPaneState, bool, bool, Option<bool>)> {
        if let TerminalEvent::PaneDied { pane_id, .. } = ev {
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

    pub(in crate::server) fn emit_pane_state_update(
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

    pub(in crate::server) fn emit_event(
        &mut self,
        event: crate::protocol::api::schema::EventEnvelope,
    ) {
        self.event_hub.push(event);
    }

    pub(in crate::server) fn emit_pane_updated(
        &mut self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) {
        if let Some(pane) = self.pane_info(ws_idx, pane_id) {
            self.emit_event(crate::protocol::api::schema::EventEnvelope {
                event: crate::protocol::api::schema::EventKind::PaneUpdated,
                data: crate::protocol::api::schema::EventData::PaneUpdated { pane },
            });
        }
    }

    pub(in crate::server) fn sync_focus_events(&mut self) {
        self.sync_focus_events_with_outer_event(None);
    }

    pub(in crate::server) fn accept_current_focus_without_events(&mut self) {
        self.last_focus = self.state.active.and_then(|idx| {
            self.state
                .workspaces
                .get(idx)
                .and_then(|workspace| workspace.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
    }

    pub(in crate::server) fn accept_current_focus_with_api_events(&mut self) {
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

    fn emit_focus_api_events(&mut self, ws_idx: usize, pane_id: crate::utils::ids::PaneId) {
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
        outer_event: Option<crate::terminal::vt::FocusEvent>,
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
            self.send_pane_focus_event(ws_idx, pane_id, crate::terminal::vt::FocusEvent::Lost);
        }
        if let Some((ws_idx, pane_id)) = current_focus {
            let event = outer_event.unwrap_or_else(|| {
                if self.state.outer_terminal_focus == Some(false) {
                    crate::terminal::vt::FocusEvent::Lost
                } else {
                    crate::terminal::vt::FocusEvent::Gained
                }
            });
            self.send_pane_focus_event(ws_idx, pane_id, event);
            self.emit_focus_api_events(ws_idx, pane_id);
        }

        self.last_focus = current_focus;
    }

    pub(in crate::server) fn send_pane_focus_event(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
        event: crate::terminal::vt::FocusEvent,
    ) {
        let Some(runtime) = self.state.workspaces.get(ws_idx).and_then(|_| {
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        }) else {
            return;
        };
        runtime.try_send_focus_event(event);
    }
}

impl App {
    pub(in crate::server) fn collect_panes_for_workspace(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<Vec<crate::protocol::api::schema::PaneInfo>, (String, String)> {
        if let Some(workspace_id) = workspace_id {
            let Some(ws_idx) = self.parse_workspace_id(workspace_id) else {
                return Err((
                    "workspace_not_found".into(),
                    format!("workspace {workspace_id} not found"),
                ));
            };
            let Some(ws) = self.state.workspaces.get(ws_idx) else {
                return Err((
                    "workspace_not_found".into(),
                    format!("workspace {workspace_id} not found"),
                ));
            };
            Ok(ws
                .tabs
                .iter()
                .flat_map(|tab| tab.layout.pane_ids().into_iter())
                .filter_map(|pane_id| self.pane_info(ws_idx, pane_id))
                .collect())
        } else {
            Ok(self
                .state
                .workspaces
                .iter()
                .enumerate()
                .flat_map(|(ws_idx, ws)| {
                    ws.tabs
                        .iter()
                        .flat_map(|tab| tab.layout.pane_ids().into_iter())
                        .filter_map(move |pane_id| self.pane_info(ws_idx, pane_id))
                })
                .collect())
        }
    }

    pub(in crate::server) fn tab_info(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<crate::protocol::api::schema::TabInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.get(tab_idx)?;
        let (agg_state, seen) = tab
            .panes
            .values()
            .filter_map(|pane| {
                self.state
                    .terminals
                    .get(&pane.attached_terminal_id)
                    .map(|terminal| (terminal.state, pane.seen))
            })
            .max_by_key(|(state, seen)| tab_attention_priority(*state, *seen))
            .unwrap_or((crate::agents::AgentState::Unknown, true));
        Some(crate::protocol::api::schema::TabInfo {
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            workspace_id: self.public_workspace_id(ws_idx),
            number: tab.number,
            label: ws.tab_display_name(tab_idx)?,
            focused: self.state.active == Some(ws_idx) && ws.active_tab == tab_idx,
            pane_count: tab.panes.len(),
            agent_status: pane_agent_status(agg_state, seen),
        })
    }

    pub(in crate::server::api) fn emit_workspace_open_events(&mut self, ws_idx: usize) {
        let workspace_info = self.workspace_info(ws_idx);
        let Some(tab) = self.tab_info(ws_idx, 0) else {
            return;
        };
        let Some(root_pane) = self.root_pane_info(ws_idx, 0) else {
            return;
        };
        self.emit_event(EventEnvelope {
            event: EventKind::WorkspaceCreated,
            data: EventData::WorkspaceCreated {
                workspace: workspace_info,
            },
        });
        self.emit_tab_and_pane_created_events(tab, root_pane);
        self.emit_layout_updated_event(ws_idx, 0);
    }

    pub(in crate::server::api) fn emit_tab_created_events(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
    ) {
        let Some(tab) = self.tab_info(ws_idx, tab_idx) else {
            return;
        };
        let Some(root_pane) = self.root_pane_info(ws_idx, tab_idx) else {
            return;
        };
        self.emit_tab_and_pane_created_events(tab, root_pane);
        self.emit_layout_updated_event(ws_idx, tab_idx);
    }

    fn emit_tab_and_pane_created_events(
        &mut self,
        tab: crate::protocol::api::schema::TabInfo,
        root_pane: crate::protocol::api::schema::PaneInfo,
    ) {
        self.emit_event(EventEnvelope {
            event: EventKind::TabCreated,
            data: EventData::TabCreated { tab },
        });
        self.emit_event(EventEnvelope {
            event: EventKind::PaneCreated,
            data: EventData::PaneCreated { pane: root_pane },
        });
    }

    pub(in crate::server) fn workspace_created_result(
        &self,
        ws_idx: usize,
    ) -> Option<crate::protocol::api::schema::ResponseResult> {
        Some(
            crate::protocol::api::schema::ResponseResult::WorkspaceCreated {
                workspace: self.workspace_info(ws_idx),
                tab: self.tab_info(ws_idx, 0)?,
                root_pane: self.root_pane_info(ws_idx, 0)?,
            },
        )
    }

    pub(in crate::server) fn tab_created_result(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<crate::protocol::api::schema::ResponseResult> {
        Some(crate::protocol::api::schema::ResponseResult::TabCreated {
            tab: self.tab_info(ws_idx, tab_idx)?,
            root_pane: self.root_pane_info(ws_idx, tab_idx)?,
        })
    }

    pub(in crate::server) fn root_pane_info(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<crate::protocol::api::schema::PaneInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.get(tab_idx)?;
        self.pane_info(ws_idx, tab.root_pane)
    }

    pub(in crate::server) fn pane_info(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<crate::protocol::api::schema::PaneInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane = ws.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
        let tab_idx = ws.find_tab_index_for_pane(pane_id)?;
        let scroll = self
            .state
            .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
            .and_then(|runtime| runtime.scroll_metrics())
            .map(|metrics| crate::protocol::api::schema::PaneScrollInfo {
                offset_from_bottom: metrics.offset_from_bottom as u64,
                max_offset_from_bottom: metrics.max_offset_from_bottom as u64,
                viewport_rows: metrics.viewport_rows as u64,
            });
        let focused = self.state.active == Some(ws_idx)
            && ws.active_tab == tab_idx
            && ws
                .focused_pane_id()
                .is_some_and(|focused| focused == pane_id);
        Some(crate::protocol::api::schema::PaneInfo {
            pane_id: self.public_pane_id(ws_idx, pane_id)?,
            terminal_id: terminal.id.to_string(),
            workspace_id: self.public_workspace_id(ws_idx),
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            focused,
            cwd: ws.tabs[tab_idx]
                .cwd_for_pane(pane_id, &self.state.terminals, &self.terminal_runtimes)
                .map(|cwd| cwd.display().to_string()),
            foreground_cwd: ws.tabs[tab_idx]
                .foreground_cwd_for_pane(pane_id, &self.terminal_runtimes)
                .map(|cwd| cwd.display().to_string()),
            label: terminal.manual_label.clone(),
            agent: terminal.effective_agent_label().map(str::to_string),
            title: None,
            terminal_title: terminal.terminal_title.clone(),
            terminal_title_stripped: terminal.terminal_title_stripped(),
            display_agent: None,
            agent_status: pane_agent_status(terminal.state, pane.seen),
            state_labels: Default::default(),
            tokens: Default::default(),
            agent_session: terminal_agent_session_info(terminal),
            scroll,
            revision: terminal.revision,
        })
    }

    pub(in crate::server) fn workspace_info(
        &self,
        index: usize,
    ) -> crate::protocol::api::schema::WorkspaceInfo {
        let ws = &self.state.workspaces[index];
        let (agg_state, seen) = ws.aggregate_state(&self.state.terminals);
        crate::protocol::api::schema::WorkspaceInfo {
            workspace_id: self.public_workspace_id(index),
            number: index + 1,
            label: ws.display_name_from(&self.state.terminals, &self.terminal_runtimes),
            focused: self.state.active == Some(index),
            pane_count: ws.public_pane_numbers.len(),
            tab_count: ws.tabs.len(),
            active_tab_id: self.public_tab_id(index, ws.active_tab).unwrap_or_else(|| {
                crate::server::workspaces::public_tab_id_for_number(&ws.id, ws.active_tab + 1)
            }),
            agent_status: pane_agent_status(agg_state, seen),
            tokens: Default::default(),
        }
    }
}

fn terminal_agent_session_info(
    terminal: &crate::terminal::TerminalState,
) -> Option<crate::protocol::api::schema::AgentSessionInfo> {
    terminal.persisted_agent_session.as_ref().map(|session| {
        crate::protocol::api::schema::AgentSessionInfo {
            source: session.source.clone(),
            agent: session.agent.clone(),
            kind: crate::server::terminals::api_session_kind(session.session_ref.kind),
            value: session.session_ref.value.clone(),
        }
    })
}
