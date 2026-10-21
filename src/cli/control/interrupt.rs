//! `agent interrupt`: Bus presses the stop key; the CLI confirms each turn
//! ended, then sends `--text` as a normal new message.
use super::{next_request_id, ParsedCommand};
use crate::messaging::control::{Request, Response};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(200);
/// Claude Code and Codex went idle within two seconds of the key, live.
const STOP_DEADLINE: Duration = Duration::from_secs(15);

pub(super) fn run(
    command: ParsedCommand,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) -> Response {
    let mut command = command;
    let text = command
        .params
        .as_object_mut()
        .and_then(|params| params.remove("text"));
    let author = command.params.get("as").cloned();
    let single = command.params.get("agent").is_some();
    let receipt = super::execute(command, &mut *send);
    if !receipt.ok {
        return receipt;
    }
    let id = receipt.id.clone();
    let room = receipt.result["room_id"].clone();
    let mut rows = receipt.result["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    wait_for_stops(&mut rows, send, pause);
    let failures: Vec<String> = rows
        .iter()
        .filter(|row| row["outcome"] == "failed")
        .map(|row| row["reason"].as_str().unwrap_or("unknown").to_owned())
        .collect();
    let mut result = if single && rows.len() == 1 {
        let mut row = rows[0].clone();
        row["room_id"] = room.clone();
        row
    } else {
        json!({"room_id": room, "agents": rows})
    };
    // Agents that did stop get --text even when another one failed: their
    // turn and queue are closed already, so it is the only work they have.
    let recipients: Vec<String> = rows
        .iter()
        .filter(|row| row["outcome"] == "stopped" || row["outcome"] == "already_idle")
        .map(|row| row["agent_id"].to_string())
        .collect();
    let finish = |result: Value| {
        if failures.is_empty() {
            return Response::success(&id, result);
        }
        let mut failure = Response::failure(&id, "interrupt_failed", failures.join("; "));
        failure.result = result;
        failure
    };
    let Some(text) = text.filter(|_| !recipients.is_empty()) else {
        return finish(result);
    };
    let mut params = json!({"room": room.to_string(), "to": recipients, "text": text, "files": []});
    if let Some(author) = author {
        params["as"] = author;
    }
    let sent = super::execute(
        ParsedCommand {
            method: "message.send",
            params,
            request_id: next_request_id(),
            wait_timeout: None,
            wait_for_message: false,
            follow: false,
        },
        &mut *send,
    );
    if !sent.ok {
        // Keep the send's transport outcome beside the interrupt rows.
        let applied = sent.result.get("applied").cloned();
        let mut failure = sent;
        failure.id = id.clone();
        failure.result = result;
        if let Some(applied) = applied {
            failure.result["applied"] = applied;
        }
        return failure;
    }
    result["message_id"] = sent.result["message_id"].clone();
    finish(result)
}

/// Reads state until every `interrupting` row's agent is idle with its
/// interrupt consumed (`stopped`), became unavailable or was deleted
/// (`failed`), or the deadline passed (`failed`).
fn wait_for_stops(
    rows: &mut [Value],
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) {
    // Pauses and reads both count, so a slow or failing read cannot stretch
    // the deadline; each read is bounded by what is left of it.
    let mut waited = Duration::ZERO;
    let mut observed_working = false;
    let interrupting = |row: &Value| row["outcome"] == "interrupting";
    while rows.iter().any(interrupting) {
        let request = Request {
            id: next_request_id(),
            method: "state".into(),
            params: json!({}),
        };
        let remaining = STOP_DEADLINE.saturating_sub(waited).max(POLL);
        let read_started = Instant::now();
        let read = send(&request, Some(remaining)).and_then(state_agents);
        waited += read_started.elapsed();
        // A missed read only delays the check; the deadline still holds.
        if let Ok(agents) = &read {
            for row in rows.iter_mut().filter(|row| interrupting(row)) {
                let agent = agents.iter().find(|a| a["id"] == row["agent_id"]);
                let name = row["name"].as_str().unwrap_or("unknown").to_owned();
                match agent {
                    None => fail(row, format!("agent {name} was deleted")),
                    Some(a) if a["status"] == "idle" && a["interrupt_pending"] != true => {
                        row["outcome"] = json!("stopped");
                    }
                    Some(a) if a["status"] == "unavailable" => {
                        fail(row, format!("agent {name} became unavailable"));
                    }
                    Some(_) => observed_working = true,
                }
            }
        }
        if !rows.iter().any(interrupting) {
            break;
        }
        if waited >= STOP_DEADLINE {
            for row in rows.iter_mut().filter(|row| interrupting(row)) {
                let reason = match &read {
                    Err(error) if !observed_working => format!(
                        "could not confirm agent {} stopped within {}s: state read failed: {error}; if it stops later, its turn closes as interrupted",
                        row["name"].as_str().unwrap_or("unknown"),
                        STOP_DEADLINE.as_secs(),
                    ),
                    _ => format!(
                        "agent {} was still working {}s after {}; if it stops later, its turn closes as interrupted",
                        row["name"].as_str().unwrap_or("unknown"),
                        STOP_DEADLINE.as_secs(),
                        row["key"].as_str().unwrap_or("the stop key"),
                    ),
                };
                fail(row, reason);
            }
            break;
        }
        pause(POLL);
        waited += POLL;
    }
}

/// Only a successful, complete agent list can establish that an agent is gone.
fn state_agents(response: Response) -> Result<Vec<Value>, String> {
    if !response.ok {
        return Err(response.error.map_or_else(
            || "state request failed".into(),
            |error| format!("{}: {}", error.code, error.message),
        ));
    }
    response.result["agents"]
        .as_array()
        .cloned()
        .ok_or_else(|| "state response has no agents array".into())
}

fn fail(row: &mut Value, reason: String) {
    row["outcome"] = json!("failed");
    row["reason"] = json!(reason);
}
