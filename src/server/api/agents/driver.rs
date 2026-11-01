//! Universal terminal driver API; works before, during and after an agent.
use crate::protocol::api::schema::{
    AgentObserveParams, AgentPressParams, ResponseResult, TerminalObservation,
};
use crate::server::api::errors::{encode_error, encode_error_body, encode_success};
use crate::server::app::App;
use crate::terminal::driver;

#[cfg(test)]
#[path = "tests/driver_test.rs"]
mod tests;

impl App {
    pub(in crate::server::api) fn handle_agent_observe(
        &mut self,
        id: String,
        params: AgentObserveParams,
    ) -> String {
        let resolved = match self.resolve_terminal_target(&params.target) {
            Ok(resolved) => resolved,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        match self.driver_observation(&resolved) {
            Ok(observation) => encode_success(id, ResponseResult::TerminalObserved { observation }),
            Err(error) => encode_error(id, "terminal_observation_unavailable", error),
        }
    }

    pub(in crate::server::api) fn handle_agent_press(
        &mut self,
        id: String,
        params: AgentPressParams,
    ) -> String {
        if let Err(error) = driver::validate_press(&params) {
            return encode_error(id, "invalid_params", error);
        }
        let resolved = match self.resolve_terminal_target(&params.target) {
            Ok(resolved) => resolved,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        if params
            .expected_terminal_id
            .as_ref()
            .is_some_and(|expected| *expected != resolved.terminal_id)
        {
            return encode_error(
                id,
                "terminal_identity_changed",
                "Terminal changed; observe the intended pane again",
            );
        }
        let observation = match self.driver_observation(&resolved) {
            Ok(observation) => observation,
            Err(error) => return encode_error(id, "terminal_observation_unavailable", error),
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return encode_error(
                id,
                "terminal_unavailable",
                "Terminal runtime is unavailable",
            );
        };
        let input_mode = if params.select.is_some() {
            "select"
        } else if params.text.is_some() {
            "text"
        } else {
            "keys"
        };
        let pane_id = observation.pane_id.clone();
        match driver::press(runtime, params, observation) {
            Ok(press) => encode_success(id, ResponseResult::TerminalPressed { press }),
            Err((code, error)) => {
                if code == "terminal_write_failed" {
                    tracing::warn!(event = "bus.terminal.driver.enqueue_failed",
                        api_request_id = %id, terminal_id = %resolved.terminal_id,
                        pane_id = %pane_id, input_mode, reason = %error,
                        "Terminal driver input could not be queued");
                }
                encode_error(id, code, error)
            }
        }
    }

    fn driver_observation(
        &self,
        resolved: &crate::server::workspaces::ids::TerminalTarget,
    ) -> Result<TerminalObservation, String> {
        let runtime = self
            .lookup_runtime_sender(resolved.ws_idx, resolved.pane_id)
            .ok_or("Terminal runtime is unavailable")?;
        let (grid, content_revision) = driver::observe(runtime)?;
        let pane_id = self
            .public_pane_id(resolved.ws_idx, resolved.pane_id)
            .ok_or("Pane identity is unavailable")?;
        Ok(TerminalObservation {
            terminal_id: resolved.terminal_id.clone(),
            pane_id,
            content_revision,
            text: grid.text(),
            focus: driver::selection_focus(&grid).ok(),
            focus_occurrence: driver::selection_identity(&grid).map_or(0, |focus| focus.occurrence),
            grid,
        })
    }
}
