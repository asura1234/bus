use crate::api;
use crate::server::api::terminal_read::AltScreenReadConflict;
use crate::server::clients::connection::{latest_shell_client, ClientConnectionMode};
use crate::server::main_loop::{non_empty_body, HeadlessServer};
use crate::server::notifications::show::{should_forward_toast_to_clients, toast_notify_kind};
use ratatui::layout::Rect;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tracing::debug;

impl HeadlessServer {
    /// Drains API requests with shutdown awareness.
    ///
    /// During shutdown, remaining requests get a `server_unavailable` error.
    pub(in crate::server) fn drain_api_requests_with_shutdown_check(&mut self) -> bool {
        let mut changed = false;
        while !self.should_quit.load(Ordering::Acquire) {
            let Ok(msg) = self.app.api_rx.try_recv() else {
                break;
            };
            changed |= self.handle_api_request_with_shutdown_check(msg);
        }
        changed
    }

    pub(in crate::server) fn reject_queued_api_requests_for_shutdown(&mut self) {
        for _ in 0..self.app.api_rx.len() {
            let Ok(msg) = self.app.api_rx.try_recv() else {
                break;
            };
            self.handle_api_request_with_shutdown_check(msg);
        }
    }

    pub(in crate::server) fn handle_api_request_with_shutdown_check_inner(
        &mut self,
        msg: api::ApiRequestMessage,
        skip_default_workspace_for_request: bool,
    ) -> bool {
        if self.shutting_down {
            // During shutdown, respond with server_unavailable.
            let response = serde_json::to_string(&api::schema::ErrorResponse {
                id: msg.request.id,
                error: api::schema::ErrorBody {
                    code: "server_unavailable".into(),
                    message: "server is shutting down".into(),
                },
            })
            .unwrap_or_else(|_| {
                r#"{"id":"","error":{"code":"server_unavailable","message":"server is shutting down"}}"#
                    .to_string()
            });
            let _ = msg.respond_to.send(response);
            return false;
        }

        let frozen_alt_screen_read = match self.alt_screen_read_conflict(&msg.request) {
            AltScreenReadConflict::None => None,
            AltScreenReadConflict::Frozen(snapshot) => Some(snapshot),
            AltScreenReadConflict::Defer => {
                self.deferred_alt_screen_reads.push(msg);
                return false;
            }
        };

        if let api::schema::Method::NotificationShow(params) = &msg.request.method {
            let response =
                self.handle_notification_show_api(msg.request.id.clone(), params.clone());
            let _ = msg.respond_to.send(response);
            return true;
        }

        match &msg.request.method {
            api::schema::Method::ClientWindowTitleSet(params) => {
                let response = self.handle_client_window_title_api(
                    msg.request.id.clone(),
                    Some(params.title.clone()),
                );
                let _ = msg.respond_to.send(response);
                return true;
            }
            api::schema::Method::ClientWindowTitleClear(_) => {
                let response = self.handle_client_window_title_api(msg.request.id.clone(), None);
                let _ = msg.respond_to.send(response);
                return true;
            }
            _ => {}
        }

        let mut changed = api::request_changes_ui(&msg.request);
        let skip_default_workspace = skip_default_workspace_for_request
            || matches!(&msg.request.method, api::schema::Method::ServerStop(_));
        changed |= self.drain_all_internal_events_with_forwarding();

        // Capture toast and effective pane states before the API call so we can
        // forward resulting client-local notifications. API requests like
        // pane.report_agent trigger handle_internal_event internally, which
        // bypasses drain_internal_events_with_forwarding. Headless mode disables
        // local sound playback, so sound notifications need to be forwarded here.
        let toast_before = self.app.state.toast.clone();
        let pane_states_before: Vec<(
            usize,
            crate::layout::PaneId,
            crate::detect::AgentState,
            Option<String>,
        )> = {
            let terminals = &self.app.state.terminals;
            self.app
                .state
                .workspaces
                .iter()
                .enumerate()
                .flat_map(|(ws_idx, ws)| {
                    ws.tabs.iter().flat_map(move |tab| {
                        tab.panes.iter().filter_map(move |(&pane_id, pane)| {
                            terminals.get(&pane.attached_terminal_id).map(|terminal| {
                                (
                                    ws_idx,
                                    pane_id,
                                    terminal.state,
                                    terminal.effective_agent_label().map(str::to_string),
                                )
                            })
                        })
                    })
                })
                .collect()
        };

        self.sync_foreground_client_state();
        if let Some(error) = self.agent_read_not_idle_error(&msg.request) {
            let response = serde_json::to_string(&api::schema::ErrorResponse {
                id: msg.request.id.clone(),
                error,
            })
            .unwrap_or_else(|_| "{}".to_owned());
            let _ = msg.respond_to.send(response);
            return changed;
        }
        let alt_screen_read_spec = self.alt_screen_read_spec(&msg.request);
        if matches!(
            &msg.request.method,
            api::schema::Method::AgentPrompt(_)
                | api::schema::Method::AgentPromptIfIdle(_)
                | api::schema::Method::AgentPromptIfUnbound(_)
        ) {
            let deferred_changed = self
                .app
                .handle_deferred_agent_api_request(msg.request, msg.respond_to);
            return changed | deferred_changed;
        }
        if self.foreground_client_id.is_some_and(|client_id| {
            self.clients
                .get(&client_id)
                .is_some_and(|client| matches!(client.mode, ClientConnectionMode::ClientShell))
        }) {
            self.app.state.view.terminal_area =
                Rect::new(0, 0, self.effective_size.0, self.effective_size.1);
        }
        let mut response = if matches!(
            &msg.request.method,
            api::schema::Method::ServerReloadConfig(_)
        ) {
            let report = self.reload_server_config(true);
            serde_json::to_string(&api::schema::SuccessResponse {
                id: msg.request.id.clone(),
                result: api::schema::ResponseResult::ConfigReload {
                    status: report.status,
                    diagnostics: report.diagnostics,
                },
            })
            .unwrap_or_else(|err| {
                serde_json::to_string(&api::schema::ErrorResponse {
                    id: String::new(),
                    error: api::schema::ErrorBody {
                        code: "serialization_error".into(),
                        message: err.to_string(),
                    },
                })
                .unwrap_or_else(|_| "{}".to_string())
            })
        } else {
            self.app
                .handle_api_request_after_internal_events_drained(msg.request)
        };
        if let Some(snapshot) = frozen_alt_screen_read {
            if let Ok(mut success) = serde_json::from_str::<api::schema::SuccessResponse>(&response)
            {
                if let api::schema::ResponseResult::PaneRead { read } = &mut success.result {
                    read.text = snapshot.text;
                    read.truncated = snapshot.truncated;
                    if let Ok(serialized) = serde_json::to_string(&success) {
                        response = serialized;
                    }
                }
            }
        }
        if let Some(spec) = alt_screen_read_spec {
            if let Ok(success) = serde_json::from_str::<api::schema::SuccessResponse>(&response) {
                if let api::schema::ResponseResult::PaneRead { read } = success.result {
                    let pending =
                        crate::server::terminals::scrollback_read::PendingAltScreenRead::start(
                            spec.terminal_id,
                            success.id,
                            msg.respond_to,
                            response,
                            read,
                            spec.lines,
                            spec.unwrap,
                            spec.initial,
                            spec.content_seq,
                            Instant::now(),
                        );
                    self.pending_alt_screen_reads.push(pending);
                    return changed;
                }
            }
        }
        let _ = msg.respond_to.send(response);

        // Forward new toast state only when a client-local delivery mode is selected.
        // Herdr delivery renders the toast in-frame and must not ask clients to
        // show a terminal or system notification.
        let toast_after = self.app.state.toast.clone();
        let forwarded_toast_from_state = if should_forward_toast_to_clients(
            self.app.state.toast_config.delivery,
        ) && toast_after.is_some()
            && toast_after != toast_before
        {
            if let Some(toast) = &toast_after {
                debug!(target: "bus::private_payload", title = %toast.title, body = %toast.context, "forwarding toast notification from API request");
                self.send_notify_to_foreground_client(
                    toast_notify_kind(self.app.state.toast_config.delivery)
                        .expect("toast forwarding requires a client notification kind"),
                    &toast.title,
                    non_empty_body(&toast.context),
                );
                true
            } else {
                false
            }
        } else {
            false
        };

        // Forward notifications for effective pane state changes that occurred
        // during the API request. Hook authority is already folded into
        // pane.state, so raw hook transitions must not produce separate sounds.
        for (ws_idx, pane_id, prev_state, prev_agent_label) in &pane_states_before {
            let pane_after = self
                .app
                .state
                .workspaces
                .get(*ws_idx)
                .and_then(|ws| ws.tabs.iter().find_map(|tab| tab.panes.get(pane_id)));

            let Some(pane_after) = pane_after else {
                continue;
            };
            let terminal_id = pane_after.attached_terminal_id.clone();

            let Some(terminal_after) = self.app.state.terminals.get(&terminal_id) else {
                continue;
            };

            let new_state = terminal_after.state;
            if new_state == *prev_state {
                continue;
            }

            let is_active_tab = self.app.state.pane_is_in_active_tab(*ws_idx, *pane_id);
            let suppress_active_tab_notifications =
                self.active_tab_suppresses_notifications(is_active_tab);

            let agent = terminal_after.effective_known_agent();
            let agent_label = terminal_after.effective_agent_label().map(str::to_string);

            debug!(target: "bus::server::main_loop",
                ws_idx,
                pane_id = pane_id.raw(),
                prev_state = ?prev_state,
                new_state = ?new_state,
                agent = ?agent,
                "pane effective state changed during API request, checking notification"
            );
            self.forward_semantic_agent_transition(
                *ws_idx,
                *pane_id,
                *prev_state,
                new_state,
                prev_agent_label.as_deref(),
                agent_label.as_deref(),
                agent,
            );

            if !forwarded_toast_from_state
                && self.app.state.toast_config.delay_seconds == 0
                && should_forward_toast_to_clients(self.app.state.toast_config.delivery)
            {
                if let Some(kind) =
                    crate::app::actions::notification_toast_for_state_change_with_agent_labels(
                        suppress_active_tab_notifications,
                        *prev_state,
                        new_state,
                        prev_agent_label.as_deref(),
                        agent_label.as_deref(),
                    )
                {
                    if let Some(agent_label) = self
                        .app
                        .state
                        .terminals
                        .get(&terminal_id)
                        .and_then(|terminal| terminal.effective_agent_label())
                    {
                        let event_text = match kind {
                            crate::app::state::ToastKind::NeedsAttention => "needs attention",
                            crate::app::state::ToastKind::Finished => "finished",
                            crate::app::state::ToastKind::UpdateInstalled => "updated",
                        };
                        let workspace_label = self.app.state.workspaces[*ws_idx].display_name_from(
                            &self.app.state.terminals,
                            &self.app.terminal_runtimes,
                        );
                        let context = crate::app::actions::notification_context(
                            &self.app.state.workspaces[*ws_idx],
                            &workspace_label,
                            *ws_idx,
                            *pane_id,
                        );
                        self.send_notify_to_foreground_client(
                            toast_notify_kind(self.app.state.toast_config.delivery)
                                .expect("toast forwarding requires a client notification kind"),
                            format!("{agent_label} {event_text}"),
                            non_empty_body(&context),
                        );
                    }
                }
            }
        }

        if !skip_default_workspace && latest_shell_client(&self.clients).is_some() {
            changed |= self.app.ensure_default_workspace();
        }

        changed
    }
}
