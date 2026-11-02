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
    prepare_fallback: impl FnOnce()
        -> io::Result<Box<dyn FnOnce() -> io::Result<InputCompletion> + Send>>,
    delay: std::time::Duration,
) -> Result<Option<InputCompletion>, String> {
    if agent.agent.as_deref() != Some("codex") || text.starts_with('/') {
        return fallback(native_only, message);
    }
    let native = runtime.codex_native();
    // The App thread never waits for a native mutex, socket, or handshake.
    if native
        .try_lock()
        .is_ok_and(|runtime| !runtime.has_session())
    {
        return fallback(native_only, message);
    }
    let typed = if native_only {
        None
    } else {
        Some(prepare_fallback().map_err(|error| {
            crate::server::api::errors::encode_error(
                message.to_owned(),
                "agent_prompt_failed",
                error.to_string(),
            )
        })?)
    };
    let text = text.to_owned();
    let thread = agent
        .agent_session
        .as_ref()
        .map(|s| s.value.clone())
        .unwrap_or_default();
    let message = message.to_owned();
    let (send, recv) = mpsc::channel();
    std::thread::spawn(move || {
        let result = submit(
            native,
            &thread,
            &text,
            steer,
            &message,
            deadline,
            expected_turn.as_deref(),
            typed,
        );
        let _ = send.send(result);
    });
    Ok(Some(
        InputCompletion::from(recv).with_enter_delay_allowance(delay),
    ))
}

type TypedSubmission = Box<dyn FnOnce() -> io::Result<InputCompletion> + Send>;

fn submit(
    native: std::sync::Arc<std::sync::Mutex<crate::agents::providers::codex::native::Runtime>>,
    thread: &str,
    text: &str,
    steer: bool,
    message: &str,
    deadline: Instant,
    expected_turn: Option<&str>,
    typed: Option<TypedSubmission>,
) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Native admission expired",
        ))
    } else {
        native
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prompt(thread, text, steer, message, deadline, expected_turn)
            .unwrap_or_else(|| {
                // No mutating RPC was sent: only this boundary permits typing.
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Native connection expired",
                    ));
                }
                typed.map_or_else(
                    || {
                        Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "Native channel unavailable",
                        ))
                    },
                    |submit| {
                        let completion = submit()?;
                        completion.wait_for_api(
                            (deadline + completion.enter_delay_allowance())
                                .saturating_duration_since(Instant::now()),
                        )
                    },
                )
            })
    }
    .map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            io::Error::other(crate::terminal::runtime::PromptNotShown::NotSubmitted)
        } else {
            error
        }
    })
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

pub(super) fn interrupt_job(
    runtime: &TerminalRuntime,
    agent: &AgentInfo,
    fallback: Box<dyn FnOnce() -> io::Result<()> + Send>,
) -> Box<dyn FnOnce() -> io::Result<()> + Send> {
    let native = runtime.codex_native();
    let thread = agent
        .agent_session
        .as_ref()
        .expect("validated interrupt")
        .value
        .clone();
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    Box::new(move || {
        native
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .interrupt(&thread, deadline)
            .unwrap_or_else(fallback)
    })
}
