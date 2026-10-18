//! Wait for asynchronous validation without blocking the coordinator.
use super::{next_request_id, wait::execute, ParsedCommand, Request, Response};
use std::time::{Duration, Instant};

pub(super) fn run(
    command: ParsedCommand,
    send: &mut impl FnMut(&Request, Option<Duration>) -> Result<Response, String>,
    pause: &mut impl FnMut(Duration),
) -> Response {
    let on = command.params["on"] == true;
    let sent = execute(command, &mut *send);
    if !sent.ok || !on {
        return sent;
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let response = match send(
            &Request {
                id: next_request_id(),
                method: "settings".into(),
                params: serde_json::json!({}),
            },
            None,
        ) {
            Ok(response) if response.ok => response,
            Ok(response) => return response,
            Err(error) => return Response::failure(&sent.id, "control_unavailable", error),
        };
        let status = &response.result["settings"]["feishu"];
        if status["pending"] == false {
            return if status["enabled"] == true {
                Response::success(&sent.id, serde_json::json!({"feishu":status}))
            } else {
                Response::failure(
                    &sent.id,
                    "feishu_unavailable",
                    status["error"]
                        .as_str()
                        .unwrap_or("Feishu notifications remain off"),
                )
            };
        }
        if Instant::now() >= deadline {
            return Response::failure(
                &sent.id,
                "feishu_timeout",
                "Feishu authentication is still pending; inspect bus settings",
            );
        }
        pause(Duration::from_millis(100));
    }
}
