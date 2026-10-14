//! Client commands for an already running Bus instance.
mod interrupt;
mod parse;
mod wait;
#[cfg(test)]
use super::help::{AGENT_HELP, DEV_HELP};
use parse::{parse, ParsedCommand};
use wait::{execute, follow_message};

use crate::messaging::control::{self, Request, Response};
#[cfg(test)]
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

#[cfg(test)]
#[path = "tests/control_focus_test.rs"]
mod focus_tests;

/// The exit code of `wait` and `send --async` when a message stalled, so a
/// caller can tell "stuck, look at it" from a failed command (1).
pub(crate) const STALLED_EXIT_CODE: i32 = 3;

/// Preserve a specific exit code after the failure reason is emitted in JSON,
/// so process termination does not add a second plain-text error report.
#[derive(Debug)]
struct CliExit {
    code: i32,
    message: String,
}

impl std::fmt::Display for CliExit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliExit {}

pub fn run(data_dir: &Path, args: &[String]) -> io::Result<()> {
    let result = run_with(
        args,
        &mut io::stdout().lock(),
        |request, timeout| match timeout {
            Some(timeout) => control::request_with_timeout(data_dir, request, timeout),
            None => control::request(data_dir, request),
        },
    );
    if let Err(error) = &result {
        // The JSON envelope already explains the failure. Returning it to
        // main would append Rust's plain-text error to combined tool output.
        let code = error
            .get_ref()
            .and_then(|e| e.downcast_ref::<CliExit>())
            .map_or(1, |exit| exit.code);
        std::process::exit(code);
    }
    result
}

/// Setup failures happen before the control request, but have the same JSON contract.
pub(super) fn report_setup_failure(args: &[String], error: io::Error) -> io::Result<()> {
    let generated_id = next_request_id();
    let id = parse(args, &generated_id).map_or(generated_id, |command| command.request_id);
    write_response(
        &mut io::stdout().lock(),
        &Response::failure(&id, "control_unavailable", error.to_string()),
    )?;
    std::process::exit(1)
}

fn next_request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "bus-{}-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
    )
}

fn run_with(
    args: &[String],
    output: &mut impl Write,
    mut send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
) -> io::Result<()> {
    run_with_pause(args, output, &mut send, thread::sleep)
}

fn write_response(output: &mut impl Write, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *output, response).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

/// `pause` stands in for sleeping between `send --async` status reads, so
/// tests can follow a message without real delays.
fn run_with_pause(
    args: &[String],
    output: &mut impl Write,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    mut pause: impl FnMut(Duration),
) -> io::Result<()> {
    let request_id = next_request_id();
    let response = match parse(args, &request_id) {
        Ok(command) if command.method == "agent.interrupt" => {
            interrupt::run(command, send, &mut pause)
        }
        Ok(command) => {
            let follow = command.follow;
            let sent = execute(command, &mut *send);
            if follow && sent.ok {
                let mut followed =
                    follow_message(&sent.id, &sent.result["message_id"], send, &mut pause);
                followed.id.clone_from(&sent.id);
                if !followed.result.is_object() {
                    followed.result = serde_json::json!({});
                }
                followed.result["send_receipt"] = sent.result;
                followed.result["message_id"] =
                    followed.result["send_receipt"]["message_id"].clone();
                followed
            } else {
                sent
            }
        }
        Err(message) => Response::failure(&request_id, "invalid_arguments", message),
    };
    write_response(output, &response)?;
    if response.ok {
        return Ok(());
    }
    let (code, message) = response
        .error
        .map(|error| (error.code, error.message))
        .unwrap_or_else(|| (String::new(), "Command failed".into()));
    Err(io::Error::other(match code.as_str() {
        "message_stalled" => CliExit {
            code: STALLED_EXIT_CODE,
            message,
        }
        .into(),
        "message_abandoned"
        | "message_provider_error"
        | "message_interrupted"
        | "interrupt_failed" => CliExit { code: 1, message }.into(),
        _ => Box::<dyn std::error::Error + Send + Sync>::from(message),
    }))
}

#[cfg(test)]
#[path = "control/tests_test.rs"]
mod tests;
