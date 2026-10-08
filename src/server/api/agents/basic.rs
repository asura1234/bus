//! Agent basic API operations.
use crate::protocol::api::schema::{
    AgentRenameParams, AgentStartParams, AgentTarget, ResponseResult,
};
use crate::server::api::errors::{encode_error_body, encode_success};
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_agent_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::AgentList {
                agents: self.collect_agent_infos(),
            },
        )
    }

    pub(in crate::server::api) fn handle_agent_get(
        &mut self,
        id: String,
        target: AgentTarget,
    ) -> String {
        self.reconcile_managed_agent_target(&target.target);
        let agent = match self.agent_info_for_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(in crate::server::api) fn handle_agent_focus(
        &mut self,
        id: String,
        target: AgentTarget,
    ) -> String {
        let agent = match self.focus_agent_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(in crate::server::api) fn handle_agent_rename(
        &mut self,
        id: String,
        params: AgentRenameParams,
    ) -> String {
        let agent = match self.rename_agent_target(&params.target, params.name) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_rename_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(in crate::server::api) fn handle_agent_start(
        &mut self,
        id: String,
        params: AgentStartParams,
    ) -> String {
        let (agent, argv) = match self.start_agent(params) {
            Ok(started) => started,
            Err(err) => return encode_error_body(id, self.agent_start_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentStarted { agent, argv })
    }
}
