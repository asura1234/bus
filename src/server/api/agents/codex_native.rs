//! Native Codex admission precedes all composer ownership and terminal writes.
use crate::{
    protocol::api::schema::AgentInfo,
    terminal::{pty::actor::InputCompletion, TerminalRuntime},
};
use std::{io, sync::mpsc, time::Instant};

/// Raw PTY activity can temporarily project Working after a native turn ended.
/// Only native metadata may admit this case; typed fallback must stay strict.
pub(super) fn can_check_idle(agent: &AgentInfo) -> bool {
    agent.agent.as_deref() == Some("codex")
        && agent.agent_status == crate::protocol::api::schema::AgentStatus::Working
        && agent.dialog_id.is_none()
        && !agent.launch_pending
        && agent.interactive_ready
}

pub(super) fn queue(
    runtime: &TerminalRuntime,
    agent: &AgentInfo,
    text: &str,
    steer: bool,
    expected_turn: Option<String>,
    message: &str,
    deadline: Instant,
    native_only: bool,
) -> Result<Option<InputCompletion>, String> {
    if agent.agent.as_deref() != Some("codex") || text.starts_with('/') {
        return fallback(native_only, message);
    }
    let native = runtime.codex_native();
    if !native
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .available(deadline)
    {
        return fallback(native_only, message);
    }
    let text = text.to_owned();
    let thread = agent
        .agent_session
        .as_ref()
        .map(|s| s.value.clone())
        .unwrap_or_default();
    let message = message.to_owned();
    let (send, recv) = mpsc::channel();
    std::thread::spawn(move || {
        let result = native
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prompt(
                &thread,
                &text,
                steer,
                &message,
                deadline,
                expected_turn.as_deref(),
            )
            .unwrap_or_else(|| {
                Err(io::Error::other(
                    "Codex native channel lost after dispatch; inspect the turn before resending",
                ))
            })
            .map_err(|error| {
                if error.kind() == io::ErrorKind::WouldBlock {
                    io::Error::other(crate::terminal::runtime::PromptNotShown::NotSubmitted)
                } else {
                    error
                }
            });
        let _ = send.send(result);
    });
    Ok(Some(recv.into()))
}

fn fallback(native_only: bool, message: &str) -> Result<Option<InputCompletion>, String> {
    if native_only {
        Err(crate::server::api::errors::encode_error(
            message.to_owned(),
            "agent_not_idle",
            "Agent is not idle; prompt was not sent",
        ))
    } else {
        Ok(None)
    }
}

pub(super) fn interrupt(runtime: &TerminalRuntime, agent: &AgentInfo) -> Option<io::Result<()>> {
    if agent.agent.as_deref() != Some("codex") {
        return None;
    }
    let thread = &agent.agent_session.as_ref()?.value;
    runtime
        .codex_native()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .interrupt(thread, Instant::now() + std::time::Duration::from_secs(2))
}
