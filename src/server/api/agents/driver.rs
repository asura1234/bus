//! Universal terminal driver API; works before, during and after an agent.
use crate::protocol::api::schema::{
    AgentObserveParams, AgentPressParams, ResponseResult, TerminalObservation, TerminalPressResult,
};
use crate::server::api::errors::{encode_error, encode_error_body, encode_success};
use crate::server::app::App;
use crate::terminal::driver::{self, SelectionStep};
use bytes::Bytes;

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
        if params.select.is_some()
            && (params.text.is_some() || !params.keys.is_empty() || params.submit)
        {
            return encode_error(
                id,
                "invalid_params",
                "Select cannot be combined with text, keys or submit",
            );
        }
        if params.select.is_none()
            && params.text.is_none()
            && params.keys.is_empty()
            && !params.submit
        {
            return encode_error(
                id,
                "invalid_params",
                "Press needs select, text, keys or submit",
            );
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
        let mut pending = runtime
            .pending_driver_selection
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let (mut keys, selection_pending) = if let Some(label) = &params.select {
            match selection_keys(&observation, label, pending.as_deref()) {
                Ok(step) => step,
                Err(error) => return encode_error(id, "selection_unavailable", error),
            }
        } else {
            (params.keys, false)
        };
        // Validate all keys before writing any text.
        let encoded = match crate::server::api::input_encoding::encode_api_keys(runtime, &keys) {
            Ok(encoded) => encoded,
            Err(error) => return encode_error(id, "invalid_key", error),
        };
        let text_written = params.text.as_ref().is_some_and(|text| !text.is_empty());
        if params.submit {
            keys.push("enter".into());
        }
        if let Err(error) = write_input(runtime, params.text.as_deref(), encoded, params.submit) {
            return encode_error(id, "terminal_write_failed", error);
        }
        if params.select.is_some() && !keys.is_empty() {
            *pending = if selection_pending {
                observation.focus.as_ref().map(|(_, label)| label.clone())
            } else {
                None
            };
        }
        encode_success(
            id,
            ResponseResult::TerminalPressed {
                press: TerminalPressResult {
                    keys,
                    text_written,
                    selection_pending,
                    observation,
                },
            },
        )
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
            grid,
        })
    }
}

fn selection_keys(
    observation: &TerminalObservation,
    label: &str,
    pending: Option<&str>,
) -> Result<(Vec<String>, bool), String> {
    let step = driver::selection_step(&observation.grid, label)?;
    if pending.is_some() && pending == observation.focus.as_ref().map(|(_, label)| label.as_str()) {
        return Ok((Vec::new(), true));
    }
    Ok(match step {
        SelectionStep::Move(key) => (vec![key.to_owned()], true),
        SelectionStep::Confirm => (vec!["enter".into()], false),
    })
}

fn write_input(
    runtime: &crate::terminal::TerminalRuntime,
    text: Option<&str>,
    encoded: Vec<Vec<u8>>,
    submit: bool,
) -> Result<(), String> {
    let mut bytes = text.map_or_else(Vec::new, |text| {
        driver::input::paste(text, runtime.bracketed_paste_enabled())
    });
    bytes.extend(encoded.into_iter().flatten());
    if bytes.is_empty() && !submit {
        return Ok(());
    }
    let enter = || {
        runtime.encode_terminal_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )
            .into(),
        )
    };
    if submit && text.is_some_and(|text| !text.is_empty()) {
        runtime
            .queue_user_input_submission(
                Bytes::from(bytes),
                Bytes::from(enter()),
                std::time::Duration::from_millis(150),
                None,
            )
            .map(|completion| {
                // This input API reports queueing. Explicitly detach the
                // completion so dropping it does not cancel delayed Enter.
                let _ = completion.into_receiver();
            })
            .map_err(|error| error.to_string())
    } else {
        if submit {
            bytes.extend(enter());
        }
        runtime
            .try_send_developer_bytes(Bytes::from(bytes))
            .map_err(|error| error.to_string())
    }
}
