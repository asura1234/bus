//! Failure-only diagnostics; capture startup UI without reading conversation history.
use crate::protocol::api::schema::{
    AgentStartupSnapshot, AgentStartupSnapshotParams, ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_agent_startup_snapshot(
        &self,
        id: String,
        params: AgentStartupSnapshotParams,
    ) -> String {
        let identity_changed = || {
            encode_error(
                id.clone(),
                "agent_identity_changed",
                "Launch terminal is no longer available at its recorded pane; no screen captured",
            )
        };
        let Some((ws_idx, pane_id)) = self.parse_current_public_pane_id(&params.pane_id) else {
            return identity_changed();
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return identity_changed();
        };
        if pane.terminal_id != params.expected_terminal_id {
            return identity_changed();
        }
        // A bound conversation can display message contents. It is not an empty
        // startup screen, so do not read it into a persisted failure diagnostic.
        if pane.agent_session.is_some() {
            return encode_error(
                id,
                "startup_snapshot_unavailable",
                "Provider session is already bound; conversation screen was not captured",
            );
        }
        let Some(terminal) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.terminal_id(pane_id))
            .and_then(|id| self.state.terminals.get(id))
        else {
            return identity_changed();
        };
        let Some((screen, osc_title, osc_progress, content_revision)) = self
            .lookup_runtime_sender(ws_idx, pane_id)
            .and_then(|runtime| runtime.startup_snapshot())
        else {
            return encode_error(
                id,
                "startup_snapshot_unavailable",
                "Startup screen is changing or runtime is unavailable; no screen captured",
            );
        };
        encode_success(
            id,
            ResponseResult::AgentStartupSnapshot {
                snapshot: AgentStartupSnapshot {
                    pane_id: pane.pane_id,
                    terminal_id: pane.terminal_id,
                    native_agent: pane.agent,
                    native_name: terminal.agent_name.clone(),
                    native_status: pane.agent_status,
                    launch_pending: terminal.managed_agent_launch_pending(),
                    interactive_ready: terminal.managed_agent_interactive_ready(),
                    screen,
                    osc_title,
                    osc_progress,
                    content_revision,
                },
            },
        )
    }
}
