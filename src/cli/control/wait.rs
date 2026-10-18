//! Async following and bounded or unbounded message waits.
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
/// Four consecutive failed reads allow three seconds of reconnect grace.
/// Successful reads reset the budget; a healthy long turn has no deadline.
const TRANSPORT_FAILURE_LIMIT: u32 = 4;

fn bus_down_response(id: &str, message: String) -> Response {
    Response::failure(
        id,
        "bus_down",
        format!("Bus is down or unreachable after {TRANSPORT_FAILURE_LIMIT} consecutive status failures: {message}"),
    )
}

fn is_server_busy(response: &Response) -> bool {
    !response.ok
        && response
            .error
            .as_ref()
            .is_some_and(|error| error.code == "server_busy")
}

/// Status reads are idempotent even if a queued coordinator call later runs.
/// These live-server responses are overload, not failed transport.
fn is_status_overloaded(response: &Response) -> bool {
    is_server_busy(response)
        || !response.ok
            && response.error.as_ref().is_some_and(|error| {
                matches!(
                    error.code.as_str(),
                    "coordinator_timeout" | "coordinator_busy"
                )
            })
}

fn next_busy_delay(delay: Duration) -> Duration {
    (delay * 2).min(BUSY_MAX_DELAY)
}

/// `send --async`: reads the message's status until every recipient has worked
/// on it and gone idle again (`turn_ended`), with no deadline. Orchestrators
/// run it as a background tool call and are woken when it exits, so a long
/// task must never time out. A blocked recipient is not idle, so it keeps
/// waiting. It fails when a request is abandoned, an unfinished turn stalls, or the message is gone.
pub(super) fn follow_message(
    id: &str,
    message: &Value,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) -> Response {
    // The send receipt carries the id as a number; message.status takes the
    // same selector text the CLI's `message status` passes.
    let author = message.get("as").cloned();
    let message = match message.get("message").unwrap_or(message) {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut busy_delay = BUSY_FIRST_DELAY;
    let mut transport_failures = 0;
    loop {
        let mut params = json!({ "message": message });
        if let Some(author) = &author {
            params["as"] = author.clone();
        }
        let request = Request {
            id: next_request_id(),
            method: "message.status".into(),
            params,
        };
        let response = match send(&request, None) {
            // The bounded control server can drop accepted connections at its
            // overload limit. Connecting proves the endpoint was reachable;
            // retry this status read, and detect a later outage on reconnect.
            Err(message) if message.starts_with("control_io:") => {
                transport_failures = 0;
                pause(busy_delay);
                busy_delay = next_busy_delay(busy_delay);
                continue;
            }
            Err(message) => {
                transport_failures += 1;
                if transport_failures >= TRANSPORT_FAILURE_LIMIT {
                    return bus_down_response(id, message);
                }
                pause(Duration::from_secs(1));
                continue;
            }
            Ok(response) => {
                transport_failures = 0;
                response
            }
        };
        match response {
            // Following has no deadline, so overload only slows it down.
            response if is_status_overloaded(&response) => {
                pause(busy_delay);
                busy_delay = next_busy_delay(busy_delay);
            }
            response if !response.ok => return response,
            response => match async_outcome(&response.result) {
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
/// code when Bus abandoned a recipient's request (other than one settled as
/// `reply_not_captured`) or one stalled, `None` while any recipient still
/// works, is blocked, or has not started.
fn async_outcome(status: &Value) -> Option<Result<(), (&'static str, String)>> {
    let requests = status["requests"].as_array()?;
    if let Some(error) = abandonment(status, true) {
        return Some(Err(error));
    }
    let ended = |r: &Value| {
        // A settling turn ended without a reply; Bus has yet to attach why.
        let settling = r["settling"] == true;
        settled_without_reply(r)
            || r["turn_ended"] == true
                && !settling
                && (r["stage"] == "replied"
                    || !(r["uncertain_outcome"] == true && r["start_bound"] != true))
    };
    if requests.iter().all(ended) {
        return Some(Ok(()));
    }
    // A finished turn whose reply was not captured is done for --async.
    stalled(status, |r| !ended(r)).map(Err)
}

/// A request settlement closed because its turn ended without a captured
/// reply and without a provider error.
fn settled_without_reply(r: &Value) -> bool {
    r["stage"] == "abandoned" && r["reason"] == "reply_not_captured"
}

/// The first abandoned request as an error: `message_interrupted` when
/// `agent interrupt` stopped its turn, `message_provider_error` when a
/// provider error ended it (worth a retry), else `message_abandoned`.
/// An async follower takes a turn settled without a reply as done, as it does
/// any ended turn; a bounded wait needs the reply, so it fails on that too.
fn abandonment(status: &Value, follower: bool) -> Option<(&'static str, String)> {
    let abandoned = status["requests"]
        .as_array()?
        .iter()
        .find(|r| r["stage"] == "abandoned" && !(follower && settled_without_reply(r)))?;
    let agent = abandoned["agent_name"].as_str().unwrap_or("unknown");
    if abandoned["reason"] == crate::messaging::model::INTERRUPTED {
        return Some((
            "message_interrupted",
            format!(
                "agent {agent}'s turn on message {} was interrupted; the room history shows who stopped it",
                status["message_id"],
            ),
        ));
    }
    if let Some(error) = abandoned["reason"]
        .as_str()
        .and_then(|reason| reason.strip_prefix("provider_error: "))
    {
        return Some((
            "message_provider_error",
            format!(
                "The provider ended agent {agent}'s turn on message {} with an error: {error}",
                status["message_id"],
            ),
        ));
    }
    Some((
        "message_abandoned",
        format!(
            "Bus abandoned message {} for agent {agent}; inspect message status and the agent's terminal",
            status["message_id"],
        ),
    ))
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

fn remaining_time(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

pub(super) fn execute(
    command: ParsedCommand,
    mut send: impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
) -> Response {
    let waiting = command.wait_for_message;
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
    let mut transport_failures = 0;
    loop {
        let remaining = remaining_time(deadline);
        if remaining == Some(Duration::ZERO) {
            return timeout_response(&id, last_status);
        }
        let mut response = match send(&request, remaining) {
            Ok(response) => {
                transport_failures = 0;
                response
            }
            Err(message) if waiting && message.starts_with("control_io:") => {
                transport_failures = 0;
                thread::sleep(remaining.map_or(busy_delay, |d| d.min(busy_delay)));
                busy_delay = next_busy_delay(busy_delay);
                request.id = next_request_id();
                continue;
            }
            Err(message) => {
                if waiting && deadline.is_none() {
                    transport_failures += 1;
                    if transport_failures >= TRANSPORT_FAILURE_LIMIT {
                        let mut response = bus_down_response(&id, message);
                        response.result = last_status;
                        return response;
                    }
                    thread::sleep(Duration::from_secs(1));
                    request.id = next_request_id();
                    continue;
                }
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return timeout_response(&id, last_status);
                }
                let mut response = Response::failure(&id, "transport_error", message);
                response.result = last_status;
                return response;
            }
        };
        response.id.clone_from(&id);
        if is_server_busy(&response) || waiting && is_status_overloaded(&response) {
            // A wait retries until its deadline (or indefinitely); other commands a few times.
            let remaining = remaining_time(deadline);
            if remaining.is_none() && !waiting {
                if busy_retries == BUSY_RETRIES {
                    return response;
                }
                busy_retries += 1;
            }
            thread::sleep(remaining.map_or(busy_delay, |remaining| busy_delay.min(remaining)));
            busy_delay = next_busy_delay(busy_delay);
            continue;
        }
        if !response.ok || !waiting {
            return response;
        }
        let outcome = if deadline.is_some() {
            abandonment(&response.result, false).map(Err)
        } else {
            async_outcome(&response.result)
        };
        match outcome {
            Some(Ok(())) => return response,
            Some(Err((code, message))) => {
                let mut failure = Response::failure(&id, code, message);
                failure.result = response.result;
                return failure;
            }
            None => {}
        }
        if deadline.is_some() && response.result["complete"] == true {
            return response;
        }
        if response.result["waiting_on_dialog"]
            .as_array()
            .is_some_and(|agents| !agents.is_empty())
        {
            return dialog_response(&id, response.result);
        }
        // Bounded waits need the captured reply; async waits use async_outcome above.
        if let Some((code, message)) = deadline.and_then(|_| stalled(&response.result, |_| true)) {
            let mut failure = Response::failure(&id, code, message);
            failure.result = response.result;
            return failure;
        }
        last_status = response.result;
        let remaining = remaining_time(deadline);
        thread::sleep(remaining.map_or(Duration::from_millis(200), |remaining| {
            Duration::from_millis(200).min(remaining)
        }));
        // Every status read is fresh even if the server caches request outcomes.
        request.id = next_request_id();
    }
}
