//! Async following and deadline-bound message waits.
use super::{next_request_id, ParsedCommand, Request, Response};
use serde_json::{json, Value};
use std::{
    thread,
    time::{Duration, Instant},
};

/// The control server rejects a connection over its shared client limit with
/// `server_busy` before reading the request, so the same request can be sent
/// again. Retries back off from the first delay up to the cap.
const BUSY_FIRST_DELAY: Duration = Duration::from_millis(100);
const BUSY_MAX_DELAY: Duration = Duration::from_secs(2);
/// Retries of a command without a wait deadline before its overload is reported.
const BUSY_RETRIES: u32 = 5;

fn is_server_busy(response: &Response) -> bool {
    !response.ok
        && response
            .error
            .as_ref()
            .is_some_and(|error| error.code == "server_busy")
}

fn next_busy_delay(delay: Duration) -> Duration {
    (delay * 2).min(BUSY_MAX_DELAY)
}

/// `send --async`: reads the message's status until every recipient has worked
/// on it and gone idle again (`turn_ended`), with no deadline. Orchestrators
/// run it as a background tool call and are woken when it exits, so a long
/// task must never time out. A blocked recipient is not idle, so it keeps
/// waiting. It fails only when a request is abandoned or the message is gone.
pub(super) fn follow_message(
    id: &str,
    message: &Value,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) -> Response {
    // The send receipt carries the id as a number; message.status takes the
    // same selector text the CLI's `message status` passes.
    let message = match message {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut busy_delay = BUSY_FIRST_DELAY;
    loop {
        let request = Request {
            id: next_request_id(),
            method: "message.status".into(),
            params: json!({ "message": message }),
        };
        match send(&request, None) {
            // The server can be briefly unreachable, e.g. while it saves;
            // keep following rather than report a failed delivery.
            Err(_) => pause(Duration::from_secs(1)),
            // Following has no deadline, so overload only slows it down.
            Ok(response) if is_server_busy(&response) => {
                pause(busy_delay);
                busy_delay = next_busy_delay(busy_delay);
            }
            Ok(response) if !response.ok => return response,
            Ok(response) => match async_outcome(&response.result) {
                Some(Ok(())) => return response,
                Some(Err((code, message))) => {
                    let mut failure = Response::failure(id, code, message);
                    failure.result = response.result;
                    return failure;
                }
                None => pause(Duration::from_millis(200)),
            },
        }
    }
}

/// `Some(Ok)` once every recipient's turn ended, `Some(Err)` with an error
/// code when Bus abandoned a recipient's request or one stalled, `None` while
/// any recipient still works, is blocked, or has not started.
fn async_outcome(status: &Value) -> Option<Result<(), (&'static str, String)>> {
    let requests = status["requests"].as_array()?;
    if let Some(abandoned) = requests.iter().find(|r| r["stage"] == "abandoned") {
        return Some(Err((
            "message_abandoned",
            format!(
                "Bus abandoned message {} for agent {}; inspect message status and the agent's terminal",
                status["message_id"],
                abandoned["agent_name"].as_str().unwrap_or("unknown"),
            ),
        )));
    }
    if requests.iter().all(|r| r["turn_ended"] == true) {
        return Some(Ok(()));
    }
    // A finished turn whose reply was not captured is done for --async.
    stalled(status, |r| r["turn_ended"] != true).map(Err)
}

/// The first stalled request (among those `relevant`) as a `message_stalled`
/// error with its reason.
fn stalled(status: &Value, relevant: impl Fn(&Value) -> bool) -> Option<(&'static str, String)> {
    let request = status["requests"]
        .as_array()?
        .iter()
        .find(|r| r["stage"] == "stalled" && relevant(r))?;
    Some((
        "message_stalled",
        format!(
            "message {} stalled for agent {} while {}: {}; inspect message status, then fix the agent or run request recover",
            status["message_id"],
            request["agent_name"].as_str().unwrap_or("unknown"),
            request["stalled_from"].as_str().unwrap_or("unknown"),
            request["reason"].as_str().unwrap_or("unknown"),
        ),
    ))
}

fn timeout_response(id: &str, last_status: Value) -> Response {
    let mut response = Response::failure(
        id,
        "timeout",
        "The message did not complete before the wait deadline; inspect result for its last status",
    );
    response.result = last_status;
    response
}

/// A recipient waits on a dialog, so waiting longer would only run out the clock.
fn dialog_response(id: &str, last_status: Value) -> Response {
    let mut response = Response::failure(
        id,
        "agent_waiting_on_dialog",
        "A recipient is waiting on a dialog; use agent dialog, then agent choose or agent answer, then wait again",
    );
    response.result = last_status;
    response
}

pub(super) fn execute(
    command: ParsedCommand,
    mut send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
) -> Response {
    let id = command.request_id;
    let deadline = command.wait_timeout.map(|timeout| Instant::now() + timeout);
    let mut request = Request {
        id: id.clone(),
        method: command.method.into(),
        params: command.params,
    };
    let mut last_status = Value::Null;
    let mut busy_delay = BUSY_FIRST_DELAY;
    let mut busy_retries = 0;
    loop {
        let remaining = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        if remaining == Some(Duration::ZERO) {
            return timeout_response(&id, last_status);
        }
        let mut response = match send(&request, remaining) {
            Ok(response) => response,
            Err(message) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return timeout_response(&id, last_status);
                }
                let mut response = Response::failure(&id, "transport_error", message);
                response.result = last_status;
                return response;
            }
        };
        response.id.clone_from(&id);
        if is_server_busy(&response) {
            // A wait retries until its deadline; other commands a few times.
            let remaining =
                deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            if remaining.is_none() {
                if busy_retries == BUSY_RETRIES {
                    return response;
                }
                busy_retries += 1;
            }
            thread::sleep(remaining.map_or(busy_delay, |remaining| busy_delay.min(remaining)));
            busy_delay = next_busy_delay(busy_delay);
            continue;
        }
        if !response.ok || deadline.is_none() || response.result["complete"] == true {
            return response;
        }
        if response.result["waiting_on_dialog"]
            .as_array()
            .is_some_and(|agents| !agents.is_empty())
        {
            return dialog_response(&id, response.result);
        }
        // A stalled message will not complete by waiting longer.
        if let Some((code, message)) = stalled(&response.result, |_| true) {
            let mut failure = Response::failure(&id, code, message);
            failure.result = response.result;
            return failure;
        }
        last_status = response.result;
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            thread::sleep(Duration::from_millis(200).min(remaining));
        }
        // Every status read is fresh even if the server caches request outcomes.
        request.id = next_request_id();
    }
}
