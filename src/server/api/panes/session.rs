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
        let Some((_ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(agent_label) = normalize_reported_agent_label(&params.agent) else {
            return invalid_agent(id);
        };
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
            },
        );

        encode_success(id, ResponseResult::Ok {})
    }
}

fn invalid_agent(id: String) -> String {
    encode_error(id, "invalid_agent", "agent label must not be empty")
}
