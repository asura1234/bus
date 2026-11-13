use crate::protocol::api::schema::{ResponseResult, SessionSnapshot};
use crate::server::app::App;

use super::errors::encode_success;

impl App {
    pub(super) fn handle_session_snapshot(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::SessionSnapshot {
                snapshot: Box::new(self.session_snapshot()),
            },
        )
    }

    pub(crate) fn session_snapshot(&self) -> SessionSnapshot {
        let focused_workspace_id = self
            .state
            .active
            .map(|ws_idx| self.public_workspace_id(ws_idx));
        let focused_tab_id = self.state.active.and_then(|ws_idx| {
            let ws = self.state.workspaces.get(ws_idx)?;
            self.public_tab_id(ws_idx, ws.active_tab)
        });
        let focused_pane_id = self.state.active.and_then(|ws_idx| {
            let ws = self.state.workspaces.get(ws_idx)?;
            self.public_pane_id(ws_idx, ws.focused_pane_id()?)
        });

        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        let mut layouts = Vec::new();
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            workspaces.push(self.workspace_info(ws_idx));
            for tab_idx in 0..ws.tabs.len() {
                if let Some(tab) = self.tab_info(ws_idx, tab_idx) {
                    tabs.push(tab);
                }
                if let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) {
                    layouts.push(layout);
                }
            }
        }

        SessionSnapshot {
            version: crate::utils::version::version(),
            protocol: crate::protocol::wire::PROTOCOL_VERSION,
            focused_workspace_id,
            focused_tab_id,
            focused_pane_id,
            workspaces,
            tabs,
            panes: self.collect_panes_for_workspace(None).unwrap_or_default(),
            layouts,
            // Rendering shares the list's freshness rules without parsing every pane.
            agents: self.collect_cached_agent_infos(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::api::schema::{EmptyParams, Method, ResponseResult, SuccessResponse};
    use crate::{server::workspaces::Workspace, utils::config::Config};

    fn app_with_two_tabs() -> crate::server::app::App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::server::app::App::new(
            &Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        let mut workspace = Workspace::test_new("snapshot");
        workspace.test_add_tab(None);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app
    }

    #[test]
    fn session_snapshot_bootstraps_runtime_resources() {
        let mut app = app_with_two_tabs();
        let response = app.handle_api_request(crate::protocol::api::schema::Request {
            id: "req_snapshot".into(),
            method: Method::SessionSnapshot(EmptyParams::default()),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::SessionSnapshot { snapshot } = success.result else {
            panic!("expected session snapshot response");
        };
        assert_eq!(success.id, "req_snapshot");
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.tabs.len(), 2);
        assert_eq!(snapshot.panes.len(), 2);
        assert_eq!(snapshot.layouts.len(), 2);
        assert_eq!(
            snapshot.focused_workspace_id.as_deref(),
            Some(snapshot.workspaces[0].workspace_id.as_str())
        );
        assert_eq!(
            snapshot.focused_tab_id.as_deref(),
            Some(snapshot.tabs[0].tab_id.as_str())
        );
        assert_eq!(
            snapshot.focused_pane_id.as_deref(),
            Some(snapshot.panes[0].pane_id.as_str())
        );
    }

    fn app_with_snapshot_agent() -> (crate::server::app::App, crate::utils::ids::PaneId) {
        let mut app = app_with_two_tabs();
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane)
            .unwrap()
            .seen = true;
        let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        let native = app.state.terminals.get_mut(&terminal).unwrap();
        native.set_agent_name("snapshot-worker".into());
        native.set_detected_state(
            Some(crate::agents::AgentKind::Claude),
            crate::agents::AgentState::Idle,
        );
        let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"ready");
        app.state.insert_test_runtime(pane, runtime);
        (app, pane)
    }

    fn assert_snapshot_status(
        app: &crate::server::app::App,
        expected: crate::protocol::api::schema::AgentStatus,
    ) {
        let session = app.session_snapshot();
        assert_eq!(session.agents.len(), 1);
        assert_eq!(session.agents[0].agent_status, expected);
        let shell = crate::server::rendering::snapshot::snapshot(app, "test", 1, None, None);
        assert_eq!(shell.agents.len(), 1);
        assert_eq!(shell.agents[0].agent_status, expected);
        assert_eq!(shell.agents[0].pane_id, session.agents[0].pane_id);
        assert_eq!(shell.agents[0].name, session.agents[0].name);
    }

    #[tokio::test]
    async fn session_snapshot_agent_cache_requires_publication_after_launch_and_restart() {
        use crate::agents::{AgentKind, AgentState};
        use crate::protocol::api::schema::AgentStatus;
        let (mut app, pane) = app_with_snapshot_agent();
        let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        let now = std::time::Instant::now();
        app.state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .begin_managed_agent(
                "snapshot-worker".into(),
                AgentKind::Claude,
                now,
                std::time::Duration::ZERO,
                std::time::Duration::from_secs(10),
            );
        app.state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
        assert_snapshot_status(&app, AgentStatus::Unknown);
        app.state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .reconcile_managed_agent_at(now + std::time::Duration::from_secs(1), false);
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_publish_screen_detection(AgentKind::Claude);
        assert_snapshot_status(&app, AgentStatus::Idle);
        assert!(!app.session_snapshot().agents[0].screen_detection_skipped);
        // A restored runtime must not inherit the old runtime's ready publication.
        app.state.insert_test_runtime(
            pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"restored"),
        );
        assert_snapshot_status(&app, AgentStatus::Unknown);
        assert!(app.session_snapshot().agents[0].screen_detection_skipped);
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_publish_screen_detection(AgentKind::Claude);
        assert_snapshot_status(&app, AgentStatus::Idle);
    }

    #[tokio::test]
    async fn session_snapshot_agent_cache_keeps_positive_busy_state_across_gaps() {
        use crate::agents::{AgentKind, AgentState};
        use crate::protocol::api::schema::AgentStatus;
        let (mut app, pane) = app_with_snapshot_agent();
        let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        for (native, expected) in [
            (AgentState::Working, AgentStatus::Working),
            (AgentState::Blocked, AgentStatus::Blocked),
        ] {
            app.state
                .terminals
                .get_mut(&terminal)
                .unwrap()
                .set_detected_state(Some(AgentKind::Claude), native);
            assert_snapshot_status(&app, expected);
            assert!(app.session_snapshot().agents[0].screen_detection_skipped);
        }
    }

    #[tokio::test]
    async fn session_snapshot_agent_cache_dialog_matches_fresh_viewport_after_scroll() {
        use crate::agents::{AgentKind, AgentState};
        use crate::protocol::api::schema::AgentStatus;
        let (mut app, pane) = app_with_snapshot_agent();
        let terminal = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        app.state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .set_detected_state(Some(AgentKind::Claude), AgentState::Blocked);
        let (runtime, _writes) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 200_000, b"", 2,
            );
        runtime.test_process_pty_bytes("historical output\r\n".repeat(80).as_bytes());
        runtime.test_process_pty_bytes(b"\x1b[H\x1b[JContinue? [y/n]");
        app.state.insert_test_runtime(pane, runtime);
        let runtime = app.lookup_runtime_sender(0, pane).unwrap();
        runtime.test_publish_screen_detection(AgentKind::Claude);
        let fresh = app.agent_info(0, pane).unwrap();
        assert!(fresh.dialog_id.is_some());
        assert_eq!(app.session_snapshot().agents[0].dialog_id, fresh.dialog_id);
        assert_snapshot_status(&app, AgentStatus::Blocked);
        runtime.scroll_up(60);
        assert!(app.agent_info(0, pane).unwrap().dialog_id.is_none());
        assert!(app.session_snapshot().agents[0].screen_detection_skipped);
        assert!(app.session_snapshot().agents[0].dialog_id.is_none());
        assert_snapshot_status(&app, AgentStatus::Blocked);
        runtime.test_publish_screen_detection(AgentKind::Claude);
        assert_eq!(
            app.session_snapshot().agents[0].dialog_id,
            app.agent_info(0, pane).unwrap().dialog_id
        );
        runtime.scroll_reset();
        assert!(app.session_snapshot().agents[0].screen_detection_skipped);
        runtime.test_publish_screen_detection(AgentKind::Claude);
        assert_eq!(app.session_snapshot().agents[0].dialog_id, fresh.dialog_id);
        // Closing a dialog does not expose stale Idle while its frame is pending.
        runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Hready");
        app.state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
        assert_snapshot_status(&app, AgentStatus::Unknown);
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_publish_screen_detection(AgentKind::Claude);
        let published = &app.session_snapshot().agents[0];
        assert!(published.dialog_id.is_none());
        assert_eq!(
            published.agent_status,
            app.agent_info(0, pane).unwrap().agent_status
        );
        assert_snapshot_status(&app, AgentStatus::Idle);
    }
}
