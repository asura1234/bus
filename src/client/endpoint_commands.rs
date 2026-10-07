use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::api::client::ApiClientError;
use crate::api::schema::{Request, ResponseResult};
use crate::protocol::ClientMessage;

use super::endpoint::ServerConnection;
use super::shell::ClientShellEndpointError;

const ENDPOINT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

struct QueuedCommand {
    boot_id: String,
    request: Box<Request>,
}

struct InFlightCommand {
    boot_id: String,
    request_id: String,
    response: Vec<u8>,
    sent_at: Instant,
}

pub(super) struct EndpointCommandResult {
    pub(super) boot_id: String,
    pub(super) request_id: String,
    pub(super) result: Result<ResponseResult, ClientShellEndpointError>,
}

/// Shell requests to the server run one at a time, in order. Request ids are unique, so a late
/// response to a timed-out request never matches the request that replaced it.
#[derive(Default)]
pub(super) struct EndpointCommands {
    queued: VecDeque<QueuedCommand>,
    in_flight: Option<InFlightCommand>,
}

impl EndpointCommands {
    pub(super) fn enqueue(&mut self, boot_id: String, request: Box<Request>) {
        self.queued.push_back(QueuedCommand { boot_id, request });
    }

    /// Sends the next queued request when none is in flight. Returns the ids of requests that
    /// could not be sent, which the caller cancels.
    pub(super) fn send_next(&mut self, connection: &mut ServerConnection) -> Vec<String> {
        let mut cancelled = Vec::new();
        if self.in_flight.is_some() {
            return cancelled;
        }
        while let Some(queued) = self.queued.pop_front() {
            let request_id = queued.request.id.clone();
            let request = match serde_json::to_string(&queued.request) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(%error, %request_id, "could not encode endpoint request");
                    cancelled.push(request_id);
                    continue;
                }
            };
            let message = ClientMessage::ClientShellEndpointRequest {
                boot_id: queued.boot_id.clone(),
                request,
            };
            if connection.send(&message).is_err() {
                cancelled.push(request_id);
                continue;
            }
            self.in_flight = Some(InFlightCommand {
                boot_id: queued.boot_id,
                request_id,
                response: Vec::new(),
                sent_at: Instant::now(),
            });
            break;
        }
        cancelled
    }

    pub(super) fn expire(&mut self, now: Instant) -> Option<EndpointCommandResult> {
        let command = self.in_flight.as_ref()?;
        if now.saturating_duration_since(command.sent_at) < ENDPOINT_COMMAND_TIMEOUT {
            return None;
        }
        let command = self.in_flight.take()?;
        Some(EndpointCommandResult {
            boot_id: command.boot_id,
            request_id: command.request_id,
            result: Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message: "this server did not respond to the action".into(),
            }),
        })
    }

    pub(super) fn receive_chunk(
        &mut self,
        boot_id: &str,
        request_id: &str,
        final_chunk: bool,
        data: Vec<u8>,
    ) -> Option<EndpointCommandResult> {
        let in_flight = self.in_flight.as_mut()?;
        if boot_id != in_flight.boot_id || request_id != in_flight.request_id {
            return None;
        }
        in_flight.response.extend(data);
        if !final_chunk {
            return None;
        }
        let in_flight = self.in_flight.take()?;
        let result = parse_response(&in_flight.request_id, &in_flight.response);
        Some(EndpointCommandResult {
            boot_id: in_flight.boot_id,
            request_id: in_flight.request_id,
            result,
        })
    }
}

pub(super) fn parse_response(
    expected_id: &str,
    response: &[u8],
) -> Result<ResponseResult, ClientShellEndpointError> {
    let value = serde_json::from_slice(response).map_err(|error| ClientShellEndpointError {
        code: None,
        message: format!("invalid endpoint response: {error}"),
    })?;
    match crate::api::client::parse_response_value(value) {
        Ok(response) if response.id == expected_id => Ok(response.result),
        Ok(response) => Err(ClientShellEndpointError {
            code: None,
            message: format!(
                "endpoint response id {:?} did not match {expected_id:?}",
                response.id
            ),
        }),
        Err(ApiClientError::ErrorResponse(response)) if response.id == expected_id => {
            Err(ClientShellEndpointError {
                code: Some(response.error.code),
                message: response.error.message,
            })
        }
        Err(ApiClientError::ErrorResponse(response)) => Err(ClientShellEndpointError {
            code: None,
            message: format!(
                "endpoint error id {:?} did not match {expected_id:?}",
                response.id
            ),
        }),
        Err(error) => Err(ClientShellEndpointError {
            code: None,
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{ResponseResult, SuccessResponse};

    fn commands_with_in_flight() -> EndpointCommands {
        EndpointCommands {
            in_flight: Some(InFlightCommand {
                boot_id: "boot-a".into(),
                request_id: "request-a".into(),
                response: Vec::new(),
                sent_at: Instant::now(),
            }),
            ..EndpointCommands::default()
        }
    }

    #[test]
    fn chunked_response_completion_is_correlated_and_clears_the_queue() {
        let mut commands = commands_with_in_flight();
        let response = serde_json::to_string(&SuccessResponse {
            id: "request-a".into(),
            result: ResponseResult::Ok {},
        })
        .unwrap();
        let split = response.len() / 2;

        assert!(commands
            .receive_chunk(
                "boot-a",
                "request-a",
                false,
                response.as_bytes()[..split].to_vec(),
            )
            .is_none());
        assert!(commands
            .receive_chunk("boot-b", "request-a", true, Vec::new())
            .is_none());
        let completed = commands
            .receive_chunk(
                "boot-a",
                "request-a",
                true,
                response.as_bytes()[split..].to_vec(),
            )
            .unwrap();

        assert_eq!(completed.boot_id, "boot-a");
        assert_eq!(completed.request_id, "request-a");
        assert!(matches!(completed.result, Ok(ResponseResult::Ok {})));
        assert!(commands.in_flight.is_none());
    }

    #[test]
    fn large_selection_response_reassembles_without_truncation() {
        let mut commands = commands_with_in_flight();
        let selection = "selected".repeat(160_000);
        let response = serde_json::to_vec(&SuccessResponse {
            id: "request-a".into(),
            result: ResponseResult::PaneSelection {
                pane_id: "w1:p1".into(),
                text: selection.clone(),
            },
        })
        .unwrap();
        let chunk_count = response.len().div_ceil(128 * 1024);
        let mut completed = None;
        for (index, chunk) in response.chunks(128 * 1024).enumerate() {
            completed = commands.receive_chunk(
                "boot-a",
                "request-a",
                index + 1 == chunk_count,
                chunk.to_vec(),
            );
        }

        assert!(matches!(
            completed.expect("final selection response").result,
            Ok(ResponseResult::PaneSelection { text, .. }) if text == selection
        ));
    }

    #[test]
    fn in_flight_endpoint_command_expires_and_ignores_its_late_response() {
        let mut commands = commands_with_in_flight();
        let expired = commands
            .expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT)
            .expect("expired endpoint command");

        assert_eq!(expired.boot_id, "boot-a");
        assert_eq!(expired.request_id, "request-a");
        assert!(matches!(
            expired.result,
            Err(ClientShellEndpointError {
                code: Some(code),
                ..
            }) if code == "endpoint_timeout"
        ));
        assert!(commands.in_flight.is_none());
        assert!(commands
            .expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT)
            .is_none());
        let late_response = serde_json::to_vec(&SuccessResponse {
            id: "request-a".into(),
            result: ResponseResult::Ok {},
        })
        .unwrap();
        assert!(commands
            .receive_chunk("boot-a", "request-a", true, late_response)
            .is_none());
    }
}
