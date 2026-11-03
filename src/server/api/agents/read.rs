//! Agent read API operations.
use super::{agent_not_found, agent_not_ready};
use crate::protocol::api::schema::{AgentSendKeysParams, PaneReadResult, ResponseResult};
use crate::server::api::errors::{encode_error, encode_error_body, encode_success};
use crate::server::app::App;
use bytes::Bytes;

impl App {
    pub(in crate::server::api) fn handle_agent_read(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::AgentReadParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &params.target);
        };
        let snapshot = crate::server::api::input_encoding::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        let Some(tab_id) = self.public_tab_id(resolved.ws_idx, resolved.tab_idx) else {
            return agent_not_found(id, &params.target);
        };

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: self
                        .public_pane_id(resolved.ws_idx, resolved.pane_id)
                        .unwrap_or_else(|| params.target.clone()),
                    workspace_id,
                    tab_id,
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: snapshot.revision,
                    truncated: snapshot.truncated,
                    viewport_rows: snapshot.viewport_rows,
                    viewport_columns: snapshot.viewport_columns,
                    requested_lines: snapshot.requested_lines,
                    returned_lines: snapshot.returned_lines,
                    available_lines: snapshot.available_lines,
                    exhausted: snapshot.exhausted,
                    note: None,
                },
            },
        )
    }

    pub(in crate::server::api) fn handle_agent_send_keys(
        &mut self,
        id: String,
        params: AgentSendKeysParams,
    ) -> String {
        self.prepare_agent_send_keys(id, params)
            .unwrap_or_else(|response| Box::new(|| response))()
    }

    pub(super) fn prepare_agent_send_keys(
        &mut self,
        id: String,
        params: AgentSendKeysParams,
    ) -> Result<Box<dyn FnOnce() -> String + Send>, String> {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return Err(encode_error_body(id, self.agent_target_error_body(err))),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return Err(agent_not_found(id, &params.target));
        };
        let Some(expected_agent) = self
            .state
            .terminals
            .get(terminal_id)
            .and_then(|terminal| terminal.effective_known_agent())
        else {
            return Err(agent_not_ready(id, &params.target));
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if !crate::server::terminals::agents::runtime_hosts_agent(runtime, expected_agent) {
            return Err(agent_not_ready(id, &params.target));
        }
        if let Some(expected) = &params.native_interrupt {
            let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) else {
                return Err(agent_not_ready(id, &params.target));
            };
            if agent.terminal_id != expected.expected_terminal_id
                || agent.agent_session.as_ref().map(|s| s.value.as_str())
                    != Some(expected.expected_session_id.as_str())
                || expected.expected_session_id.is_empty()
                || agent.agent.as_deref() != Some("codex")
                || params.keys != ["esc"]
            {
                return Err(agent_not_ready(id, &params.target));
            }
            return agent_keys_job(runtime, id, params.keys, Some(agent));
        }
        agent_keys_job(runtime, id, params.keys, None)
    }
}

pub(super) enum KeyCompletion {
    Typed(std::io::Result<()>),
    Native(std::io::Result<crate::terminal::runtime::native_input::NativeInterruptCompletion>),
}

pub(super) type KeyJob = Box<dyn FnOnce() -> KeyCompletion + Send>;

fn agent_keys_job(
    runtime: &crate::terminal::TerminalRuntime,
    id: String,
    keys: Vec<String>,
    native: Option<crate::protocol::api::schema::AgentInfo>,
) -> Result<Box<dyn FnOnce() -> String + Send>, String> {
    let encoded = crate::server::api::input_encoding::encode_api_keys(runtime, &keys)
        .map_err(|key| encode_error(id.clone(), "invalid_key", format!("unsupported key {key}")))?;
    let bytes = Bytes::from(encoded.into_iter().flatten().collect::<Vec<u8>>());
    let sender = runtime.submission_handle(true);
    let submit: KeyJob =
        Box::new(move || KeyCompletion::Typed(sender.try_send_developer_bytes(bytes)));
    let identity = native.as_ref().map(|agent| {
        (
            agent.terminal_id.clone(),
            agent
                .agent_session
                .as_ref()
                .expect("validated interrupt")
                .value
                .clone(),
        )
    });
    #[cfg(unix)]
    let submit = if let Some(agent) = native {
        super::codex_native::interrupt_job(runtime, &agent, submit)
    } else {
        submit
    };
    #[cfg(not(unix))]
    let _ = native;
    Ok(Box::new(move || match submit() {
        KeyCompletion::Typed(Ok(())) => encode_success(id, ResponseResult::Ok {}),
        KeyCompletion::Typed(Err(err)) => {
            encode_error(id, "agent_send_keys_failed", err.to_string())
        }
        KeyCompletion::Native(Ok(completed)) => {
            let (terminal_id, session_id) = identity.expect("validated native interrupt");
            encode_success(
                id,
                ResponseResult::AgentInterrupted {
                    terminal_id,
                    session_id,
                    turn_id: completed.turn_id,
                },
            )
        }
        KeyCompletion::Native(Err(_)) => encode_error(
            id,
            "agent_interrupt_unconfirmed",
            "Native interrupt is unconfirmed; work may still be running",
        ),
    }))
}
