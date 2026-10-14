//! `agent interrupt`: Bus presses the stop key; the CLI confirms each turn
//! ended, then sends `--text` as a normal new message.
use super::{next_request_id, ParsedCommand};
use crate::messaging::control::{Request, Response};
use serde_json::{json, Value};
use std::time::Duration;

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
    if !failures.is_empty() {
        let mut failure = Response::failure(&id, "interrupt_failed", failures.join("; "));
        failure.result = result;
        return failure;
    }
    let Some(text) = text else {
        return Response::success(&id, result);
    };
    let recipients: Vec<String> = rows
        .iter()
        .filter(|row| row["outcome"] == "stopped" || row["outcome"] == "already_idle")
        .map(|row| row["agent_id"].to_string())
        .collect();
    if recipients.is_empty() {
        return Response::success(&id, result);
    }
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
        let mut failure = sent;
        failure.id = id;
        failure.result = result;
        return failure;
    }
    result["message_id"] = sent.result["message_id"].clone();
    Response::success(&id, result)
}

/// Reads state until every `interrupting` row's agent is idle with its
/// interrupt consumed (`stopped`), became unavailable or was deleted
/// (`failed`), or the deadline passed (`failed`).
fn wait_for_stops(
    rows: &mut [Value],
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) {
    let mut waited = Duration::ZERO;
    let interrupting = |row: &Value| row["outcome"] == "interrupting";
    while rows.iter().any(interrupting) {
        let request = Request {
            id: next_request_id(),
            method: "state".into(),
            params: json!({}),
        };
        // A missed read only delays the check; the deadline still holds.
        if let Ok(state) = send(&request, None).map(|response| response.result) {
            for row in rows.iter_mut().filter(|row| interrupting(row)) {
                let agent = state["agents"]
                    .as_array()
                    .and_then(|agents| agents.iter().find(|a| a["id"] == row["agent_id"]));
                let name = row["name"].as_str().unwrap_or("unknown").to_owned();
                match agent {
                    None => fail(row, format!("agent {name} was deleted")),
                    Some(a) if a["status"] == "idle" && a["interrupt_pending"] != true => {
                        row["outcome"] = json!("stopped");
                    }
                    Some(a) if a["status"] == "unavailable" => {
                        fail(row, format!("agent {name} became unavailable"));
                    }
                    Some(_) => {}
                }
            }
        }
        if !rows.iter().any(interrupting) {
            break;
        }
        if waited >= STOP_DEADLINE {
            for row in rows.iter_mut().filter(|row| interrupting(row)) {
                let reason = format!(
                    "agent {} was still working {}s after {}; if it stops later, its turn closes as interrupted",
                    row["name"].as_str().unwrap_or("unknown"),
                    STOP_DEADLINE.as_secs(),
                    row["key"].as_str().unwrap_or("the stop key"),
                );
                fail(row, reason);
            }
            break;
        }
        pause(POLL);
        waited += POLL;
    }
}

fn fail(row: &mut Value, reason: String) {
    row["outcome"] = json!("failed");
    row["reason"] = json!(reason);
}
