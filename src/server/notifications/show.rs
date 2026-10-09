use crate::agents::AgentState;
use crate::protocol;
use crate::server::app_state::AppState;
use crate::terminal::TerminalRuntimeRegistry;
use crate::utils::config;
use crate::utils::ids::PaneId;

pub(crate) fn should_forward_toast_to_clients(delivery: config::ToastDelivery) -> bool {
    toast_notify_kind(delivery).is_some()
}

pub(crate) fn toast_notify_kind(
    delivery: config::ToastDelivery,
) -> Option<protocol::wire::NotifyKind> {
    match delivery {
        config::ToastDelivery::Terminal => Some(protocol::wire::NotifyKind::Toast),
        config::ToastDelivery::System => Some(protocol::wire::NotifyKind::SystemToast),
        config::ToastDelivery::Off | config::ToastDelivery::Bus => None,
    }
}

pub(crate) fn toast_message_from_state_change(
    state: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    pane_id: PaneId,
    suppress_active_tab_notifications: bool,
    prev_state: AgentState,
    new_state: AgentState,
    previous_agent_label: Option<&str>,
) -> Option<String> {
    state
        .workspaces
        .iter()
        .enumerate()
        .find_map(|(ws_idx, ws)| {
            ws.tabs.iter().find_map(|tab| {
                let pane = tab.panes.get(&pane_id)?;
                let agent_label = state
                    .terminals
                    .get(&pane.attached_terminal_id)
                    .and_then(|terminal| terminal.effective_agent_label())
                    .or(previous_agent_label)?;
                let kind = crate::server::notifications::policy::notification_toast_for_state_change_with_agent_labels(
                    suppress_active_tab_notifications,
                    prev_state,
                    new_state,
                    previous_agent_label,
                    Some(agent_label),
                )?;
                let workspace_label = ws.display_name_from(&state.terminals, terminal_runtimes);
                Some(format!(
                    "{} {}: {}",
                    agent_label,
                    toast_event_text(kind),
                    crate::server::notifications::policy::notification_context(ws, &workspace_label, ws_idx, pane_id)
                ))
            })
        })
}

fn toast_event_text(kind: crate::server::app_state::ToastKind) -> &'static str {
    match kind {
        crate::server::app_state::ToastKind::NeedsAttention => "needs attention",
        crate::server::app_state::ToastKind::Finished => "finished",
        crate::server::app_state::ToastKind::UpdateInstalled => "updated",
    }
}

use crate::server::api::errors;
use crate::server::app::{App, ToastKind};
use std::time::{Duration, Instant};

const API_NOTIFICATION_RATE_LIMIT: Duration = Duration::from_secs(1);
impl App {
    pub(crate) fn refresh_new_bus_toast_context_for_update(
        &mut self,
        update: &crate::server::terminals::events::PaneStateUpdate,
        previous_toast: &Option<crate::server::app_state::ToastNotification>,
    ) {
        if !matches!(
            self.state.toast_config.delivery,
            crate::utils::config::ToastDelivery::Bus
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
        let context = crate::server::notifications::policy::notification_context(
            ws,
            &workspace_label,
            update.ws_idx,
            update.pane_id,
        );
        if let Some(toast) = self.state.toast.as_mut() {
            toast.context = context;
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
            let context = crate::server::notifications::policy::notification_context(
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

    pub(in crate::server) fn handle_notification_show(
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
            crate::utils::config::ToastDelivery::Off => NotificationShowReason::Disabled,
            crate::utils::config::ToastDelivery::Bus => {
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
            crate::utils::config::ToastDelivery::Terminal
            | crate::utils::config::ToastDelivery::System => {
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
mod tests {
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use crate::agents::AgentKind;
    #[cfg(unix)]
    use crate::terminal::TerminalState;

    #[cfg(unix)]
    fn init_repo(path: &std::path::Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git init failed for {}", path.display());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn toast_message_uses_live_root_runtime_cwd_label() {
        let mut state = AppState::test_new();
        state
            .workspaces
            .push(crate::server::workspaces::Workspace::test_new("stale"));
        state.ensure_test_terminals();
        let root = state.workspaces[0].tabs[0].root_pane;
        let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
        let temp_root = std::env::temp_dir().join(format!(
            "bus-forwarded-toast-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stale_cwd = temp_root.join("__bus_original__");
        let live_cwd = temp_root.join("__bus_projects__");
        std::fs::create_dir_all(&stale_cwd).unwrap();
        std::fs::create_dir_all(&live_cwd).unwrap();
        init_repo(&stale_cwd);
        init_repo(&live_cwd);
        state.workspaces[0].custom_name = None;
        state.workspaces[0].identity_cwd = stale_cwd.clone();
        let mut terminal = TerminalState::new(terminal_id.clone(), stale_cwd);
        terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
        state.terminals.insert(terminal_id.clone(), terminal);
        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            root,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::utils::theme::color::TerminalTheme::default(),
            None,
            crate::terminal::runtime::spawn::PaneShellConfig::new(
                "/bin/sh",
                crate::utils::config::ShellModeConfig::NonLogin,
            ),
            &crate::terminal::runtime::spawn::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut terminal_runtimes = TerminalRuntimeRegistry::new();
        terminal_runtimes.insert(terminal_id, runtime);

        let message = toast_message_from_state_change(
            &state,
            &terminal_runtimes,
            root,
            false,
            AgentState::Working,
            AgentState::Idle,
            Some("codex"),
        );

        assert_eq!(
            message.as_deref(),
            Some("codex finished: __bus_projects__ · 1")
        );

        for (_, runtime) in terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(temp_root);
    }
}
