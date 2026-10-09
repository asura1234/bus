//! Client commands for an already running, explicitly enabled Bus developer instance.
mod parse;
mod wait;
#[cfg(test)]
use super::help::HELP;
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

/// A failure that ends the process with its own code and a one-line reason on
/// stderr, instead of the generic error report.
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
        if let Some(exit) = error.get_ref().and_then(|e| e.downcast_ref::<CliExit>()) {
            eprintln!("bus: {exit}");
            std::process::exit(exit.code);
        }
    }
    result
}

fn next_request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "bus-dev-{}-{}-{}",
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
        Ok(command) => {
            let follow = command.follow;
            let sent = execute(command, &mut *send);
            if follow && sent.ok {
                // The send receipt comes first, so a caller that stops
                // reading early still knows the message id to look up.
                write_response(output, &sent)?;
                let mut followed =
                    follow_message(&sent.id, &sent.result["message_id"], send, &mut pause);
                // Both lines answer the caller's one request id.
                followed.id.clone_from(&sent.id);
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
        .unwrap_or_else(|| (String::new(), "Developer command failed".into()));
    Err(io::Error::other(match code.as_str() {
        "message_stalled" => CliExit {
            code: STALLED_EXIT_CODE,
            message,
        }
        .into(),
        "message_abandoned" => CliExit { code: 1, message }.into(),
        _ => Box::<dyn std::error::Error + Send + Sync>::from(message),
    }))
}

#[cfg(test)]
#[path = "control/tests_test.rs"]
mod tests;
