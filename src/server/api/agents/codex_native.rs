//! Native Codex admission precedes all composer ownership and terminal writes.
use crate::{
    protocol::api::schema::AgentInfo,
    terminal::{pty::actor::InputCompletion, TerminalRuntime},
};
use std::{io, sync::mpsc, time::Instant};

pub(super) fn queue(
    runtime: &TerminalRuntime,
    agent: &AgentInfo,
    text: &str,
    steer: bool,
    expected_turn: Option<String>,
    message: &str,
    deadline: Instant,
) -> Option<InputCompletion> {
    if agent.agent.as_deref() != Some("codex") || text.starts_with('/') {
        return None;
    }
    let native = runtime.shared_codex_handle();
    if !native
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|runtime| runtime.has_session())
    {
        return None;
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
            .as_mut()
            .and_then(|runtime| {
                runtime.prompt(
                    &thread,
                    &text,
                    steer,
                    &message,
                    deadline,
                    expected_turn.as_deref(),
                )
            })
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
    Some(recv.into())
}

pub(super) fn interrupt(runtime: &TerminalRuntime, agent: &AgentInfo) -> Option<io::Result<()>> {
    if agent.agent.as_deref() != Some("codex") {
        return None;
    }
    let thread = &agent.agent_session.as_ref()?.value;
    runtime
        .shared_codex_handle()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|runtime| {
            runtime
                .interrupt(
                    thread,
                    Instant::now() + std::time::Duration::from_secs(2),
                )
                .map(|result| result.map(|_| ()))
        })
}
