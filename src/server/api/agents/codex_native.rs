//! Native Codex admission precedes all composer ownership and terminal writes.
use crate::{
    protocol::api::schema::AgentInfo,
    terminal::{
        pty::actor::{InputCompletion, SharedSubmissionTimestamps},
        TerminalRuntime,
    },
};
use std::{io, sync::mpsc, time::Instant};

/// A ready frontend can project Working during repaint or Blocked for a draft.
/// The caller must prove a live draft before bypassing its Blocked guard. Only
/// native metadata may admit either case; typed fallback must stay strict.
pub(super) fn can_check_idle(agent: &AgentInfo) -> bool {
    agent.agent.as_deref() == Some("codex")
        && matches!(
            agent.agent_status,
            crate::protocol::api::schema::AgentStatus::Working
                | crate::protocol::api::schema::AgentStatus::Blocked
        )
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
    let Some(native) = runtime.native_input() else {
        return fallback(native_only, message);
    };
    // The App thread never waits for a native mutex, socket, or handshake.
    if !native.has_session() {
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
    let timestamps: SharedSubmissionTimestamps = Default::default();
    let completion = InputCompletion::from(recv)
        .with_timestamps(timestamps.clone())
        .with_enter_delay_allowance(delay);
    completion.update_timestamps(|timestamps| {
        timestamps.writer_queued_at_ms = Some(crate::utils::time::now_ms());
    });
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
            &timestamps,
        );
        if result.is_ok() {
            // RPC success confirms native admission, without implying PTY bytes
            // or model attention. Typed fallback retains its earlier receipt.
            timestamps
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .native_confirmed_at_ms
                .get_or_insert(crate::utils::time::now_ms());
        }
        let _ = send.send(result);
    });
    Ok(Some(completion))
}

type TypedSubmission = Box<dyn FnOnce() -> io::Result<InputCompletion> + Send>;

fn submit(
    native: crate::terminal::runtime::native_input::NativeInputHandle,
    thread: &str,
    text: &str,
    steer: bool,
    message: &str,
    deadline: Instant,
    expected_turn: Option<&str>,
    typed: Option<TypedSubmission>,
    timestamps: &SharedSubmissionTimestamps,
) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Native admission expired",
        ))
    } else {
        native
            .prompt(crate::terminal::runtime::native_input::NativePrompt {
                thread,
                text,
                steer,
                message,
                deadline,
                expected_turn,
            })
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
                        let result = completion.wait_for_api(
                            (deadline + completion.enter_delay_allowance())
                                .saturating_duration_since(Instant::now()),
                        );
                        let mut submitted = completion.submission_timestamps();
                        let mut recorded =
                            timestamps.lock().unwrap_or_else(|error| error.into_inner());
                        submitted.server_accepted_at_ms = recorded
                            .server_accepted_at_ms
                            .or(submitted.server_accepted_at_ms);
                        submitted.writer_queued_at_ms = recorded
                            .writer_queued_at_ms
                            .or(submitted.writer_queued_at_ms);
                        *recorded = submitted;
                        result
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
    fallback: super::read::KeyJob,
) -> super::read::KeyJob {
    let Some(native) = runtime.native_input() else {
        return fallback;
    };
    let thread = agent
        .agent_session
        .as_ref()
        .expect("validated interrupt")
        .value
        .clone();
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    Box::new(move || match native.interrupt(&thread, deadline) {
        Some(result) => super::read::KeyCompletion::Native(result),
        None => fallback(),
    })
}
