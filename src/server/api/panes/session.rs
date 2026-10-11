use super::pane_not_found;
use crate::protocol::api::schema::{PaneReportAgentSessionParams, ResponseResult};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::api::input_encoding::normalize_reported_agent_label;
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_pane_report_agent_session(
        &mut self,
        id: String,
        params: PaneReportAgentSessionParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(agent_label) = normalize_reported_agent_label(&params.agent) else {
            return invalid_agent(id);
        };
        let pane_shell = self
            .state
            .terminal_id_for_pane(ws_idx, pane_id)
            .and_then(|terminal_id| self.terminal_runtimes.get(&terminal_id))
            .and_then(|runtime| runtime.child_pid());
        let reporter = reporter_below_pane_shell(&params.reporter, pane_shell);
        self.handle_internal_event(
            crate::terminal::events::TerminalEvent::AgentSessionReported {
                pane_id,
                session_ref: crate::agents::resume::catalog::session_ref_from_report(
                    &params.source,
                    &agent_label,
                    params.agent_session_id,
                    params.agent_session_path,
                ),
                source: params.source,
                agent_label,
                seq: params.seq,
                session_start_source:
                    crate::agents::resume::catalog::normalize_session_start_source(
                        params.session_start_source,
                    ),
                reporter,
            },
        );

        encode_success(id, ResponseResult::Ok {})
    }
}

/// The processes of the reporting agent job: the chain up to, not including, this
/// pane's shell. Everything from the shell up is shared by every job in the pane,
/// so only this part tells one agent process from its replacement. A chain that
/// does not pass through the pane shell proves nothing and is dropped.
pub(in crate::server::api) fn reporter_below_pane_shell(
    chain: &[crate::protocol::api::schema::ReportingProcess],
    pane_shell: Option<u32>,
) -> Vec<crate::platform::ProcessInstance> {
    let Some(shell_index) = pane_shell.and_then(|shell| chain.iter().position(|p| p.pid == shell))
    else {
        return Vec::new();
    };
    chain[..shell_index]
        .iter()
        .map(|process| crate::platform::ProcessInstance {
            pid: process.pid,
            birth: process.birth,
        })
        .collect()
}

fn invalid_agent(id: String) -> String {
    encode_error(id, "invalid_agent", "agent label must not be empty")
}
