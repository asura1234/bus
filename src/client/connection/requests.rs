use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::api::client::ApiClientError;
use crate::api::schema::{Request, ResponseResult};
use crate::protocol::ClientMessage;

use super::bootstrap::ServerConnection;
use crate::client::compositor::{
    ClientShellAction, ClientShellEndpointError, ClientShellInput, ClientShellState, PaneHit,
    PendingEndpointKind, PendingEndpointRequest,
};

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

pub(in crate::client) struct EndpointCommandResult {
    pub(in crate::client) boot_id: String,
    pub(in crate::client) request_id: String,
    pub(in crate::client) result: Result<ResponseResult, ClientShellEndpointError>,
}

/// Shell requests to the server run one at a time, in order. Request ids are unique, so a late
/// response to a timed-out request never matches the request that replaced it.
#[derive(Default)]
pub(in crate::client) struct EndpointCommands {
    queued: VecDeque<QueuedCommand>,
    in_flight: Option<InFlightCommand>,
}

impl EndpointCommands {
    pub(in crate::client) fn enqueue(&mut self, boot_id: String, request: Box<Request>) {
        self.queued.push_back(QueuedCommand { boot_id, request });
    }

    /// Sends the next queued request when none is in flight. Returns the ids of requests that
    /// could not be sent, which the caller cancels.
    pub(in crate::client) fn send_next(
        &mut self,
        connection: &mut ServerConnection,
    ) -> Vec<String> {
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

    pub(in crate::client) fn expire(&mut self, now: Instant) -> Option<EndpointCommandResult> {
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

    pub(in crate::client) fn receive_chunk(
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

pub(in crate::client) fn parse_response(
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

impl ClientShellState {
    pub(in crate::client) fn request_selection_copy(
        &mut self,
        outcome: &mut ClientShellInput,
        live: bool,
    ) {
        let Some(selection) = self.selection.as_ref() else {
            return;
        };
        let pane_id = selection.pane_id.clone();
        let content_revision = self
            .pane_surface
            .as_ref()
            .and_then(|surface| surface.panes.iter().find(|pane| pane.pane_id == pane_id))
            .map(|pane| pane.content_revision)
            // Read a manual mouse selection atomically from the live terminal. Output
            // between the displayed frame and this request must not reject the copy.
            .filter(|_| !live);
        let (anchor, cursor) = selection.ordered_cells();
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneSelectionRead(
                crate::api::schema::PaneSelectionReadParams {
                    pane_id,
                    anchor: crate::api::schema::PaneTextPoint {
                        row: anchor.0,
                        col: anchor.1,
                    },
                    cursor: crate::api::schema::PaneTextPoint {
                        row: cursor.0,
                        col: cursor.1,
                    },
                    content_revision,
                },
            ),
            PendingEndpointKind::SelectionCopy,
            outcome,
        );
    }

    pub(in crate::client) fn request_word_selection(
        &mut self,
        hit: &PaneHit,
        viewport_row: u16,
        col: u16,
        outcome: &mut ClientShellInput,
    ) {
        let absolute_row = crate::selection::absolute_row_for_viewport(viewport_row, hit.scroll);
        let content_revision = self
            .pane_surface
            .as_ref()
            .and_then(|surface| {
                surface
                    .panes
                    .iter()
                    .find(|pane| pane.pane_id == hit.pane_id)
            })
            .map(|pane| pane.content_revision);
        self.word_selection_generation = self.word_selection_generation.saturating_add(1);
        let generation = self.word_selection_generation;
        self.pending_word_selection = Some(generation);
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneSelectionRead(
                crate::api::schema::PaneSelectionReadParams {
                    pane_id: hit.pane_id.clone(),
                    anchor: crate::api::schema::PaneTextPoint {
                        row: absolute_row,
                        col: 0,
                    },
                    cursor: crate::api::schema::PaneTextPoint {
                        row: absolute_row,
                        col: hit.inner_rect.width.saturating_sub(1),
                    },
                    content_revision,
                },
            ),
            PendingEndpointKind::WordSelection {
                pane_id: hit.pane_id.clone(),
                absolute_row,
                col,
                generation,
            },
            outcome,
        ) {
            self.pending_word_selection = None;
        }
    }

    pub(in crate::client) fn push_endpoint_method(
        &mut self,
        method: crate::api::schema::Method,
        outcome: &mut ClientShellInput,
    ) {
        self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome);
    }

    pub(in crate::client) fn push_endpoint_method_with_kind(
        &mut self,
        method: crate::api::schema::Method,
        kind: PendingEndpointKind,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let method_name = crate::protocol::api::api_method_name(&method).to_owned();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request_id = format!("client-shell:{request_id}");
        self.pending_requests.insert(
            request_id.clone(),
            PendingEndpointRequest {
                boot_id: snapshot.boot_id.clone(),
                method_name,
                kind,
            },
        );
        outcome.actions.push(ClientShellAction::Endpoint {
            boot_id: snapshot.boot_id.clone(),
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method,
            }),
        });
        true
    }

    pub(crate) fn cancel_endpoint_request(&mut self, request_id: &str) -> bool {
        let Some(pending) = self.pending_requests.get(request_id) else {
            return false;
        };
        let boot_id = pending.boot_id.clone();
        let (repaint, actions) = self.handle_endpoint_result(
            &boot_id,
            request_id,
            Err(ClientShellEndpointError {
                code: Some("endpoint_cancelled".into()),
                message: "This server action was interrupted. Check its state before retrying."
                    .into(),
            }),
        );
        debug_assert!(
            actions.is_empty(),
            "cancellation must not start another action"
        );
        repaint
    }

    pub(crate) fn handle_endpoint_result(
        &mut self,
        boot_id: &str,
        request_id: &str,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Some(pending) = self.pending_requests.remove(request_id) else {
            return (false, Vec::new());
        };
        if pending.boot_id != boot_id
            || self
                .snapshot
                .as_deref()
                .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return (false, Vec::new());
        }
        if let Err(error) = &result {
            tracing::warn!(
                method = %pending.method_name,
                code = ?error.code,
                message = %error.message,
                "server rejected client shell request"
            );
        }
        match pending.kind {
            PendingEndpointKind::Generic => {}
            PendingEndpointKind::PaneScroll { pane_id, serial } => {
                let mut outcome = ClientShellInput::default();
                let repaint = self.complete_pane_scroll(pane_id, serial, result, &mut outcome);
                return (repaint, outcome.actions);
            }
            PendingEndpointKind::SelectionCopy => {
                return match result {
                    Ok(crate::api::schema::ResponseResult::PaneSelection { text, .. })
                        if !text.is_empty() =>
                    {
                        let repaint = self.show_copy_feedback(std::time::Instant::now());
                        (
                            repaint,
                            vec![ClientShellAction::ClipboardWrite(text.into_bytes())],
                        )
                    }
                    Ok(crate::api::schema::ResponseResult::PaneSelection { .. }) => {
                        (false, Vec::new())
                    }
                    Ok(_) => {
                        tracing::warn!("endpoint returned an unexpected selection result");
                        (true, Vec::new())
                    }
                    Err(_) => (true, Vec::new()),
                };
            }
            PendingEndpointKind::WordSelection {
                pane_id,
                absolute_row,
                col,
                generation,
            } => {
                return self.complete_word_selection(
                    pane_id,
                    absolute_row,
                    col,
                    generation,
                    result,
                );
            }
            PendingEndpointKind::PaneLinkActivate {
                pane_id,
                inner_rect,
                fallback_events,
            } => {
                return self.complete_pane_link(pane_id, inner_rect, fallback_events, result);
            }
        }
        (result.is_err(), Vec::new())
    }

    fn complete_word_selection(
        &mut self,
        pane_id: String,
        absolute_row: u32,
        col: u16,
        generation: u64,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        if self.pending_word_selection != Some(generation)
            || self
                .snapshot
                .as_deref()
                .is_none_or(|snapshot| !snapshot.panes.iter().any(|pane| pane.pane_id == pane_id))
        {
            return (false, Vec::new());
        }
        self.pending_word_selection = None;
        let row_text = match result {
            Ok(crate::api::schema::ResponseResult::PaneSelection {
                pane_id: returned_pane_id,
                text,
            }) if returned_pane_id == pane_id => text,
            Ok(crate::api::schema::ResponseResult::PaneSelection { .. }) => {
                return (false, Vec::new())
            }
            Ok(_) => {
                tracing::warn!("endpoint returned an unexpected word-selection result");
                return (true, Vec::new());
            }
            Err(_) => return (true, Vec::new()),
        };
        let Some((start_col, end_col)) =
            crate::utils::text::hit_testing::word_bounds_at_column(&row_text, col, |ch| {
                u16::from(crate::utils::text::width::unicode_codepoint_width(
                    ch as u32,
                ))
            })
        else {
            self.selection = None;
            return (true, Vec::new());
        };
        let mut selection = crate::selection::Selection::absolute_range(
            pane_id,
            (absolute_row, start_col),
            (absolute_row, end_col),
        );
        if !selection.finish() {
            return (false, Vec::new());
        }
        self.selection = Some(selection);
        self.selection_autoscroll = None;
        self.selection_autoscroll_deadline = None;
        if !self.config.copy_on_select {
            return (true, Vec::new());
        }
        self.selection_highlight_clear_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
        let mut outcome = ClientShellInput::default();
        self.request_selection_copy(&mut outcome, false);
        (true, outcome.actions)
    }

    fn complete_pane_link(
        &mut self,
        pane_id: String,
        inner_rect: ratatui::layout::Rect,
        fallback_events: Vec<crossterm::event::MouseEvent>,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let completed_before_release = !fallback_events.iter().any(|event| {
            event.kind == crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left)
        });
        let replay = self
            .hits
            .panes
            .iter()
            .any(|hit| hit.pane_id == pane_id && hit.inner_rect == inner_rect)
            .then_some(fallback_events);
        if replay.is_none() {
            self.url_click_consumes_until_up = completed_before_release;
        }
        let replay_action = |events: Option<Vec<crossterm::event::MouseEvent>>| {
            events
                .map(ClientShellAction::ReplayMouse)
                .into_iter()
                .collect()
        };
        match result {
            Ok(crate::api::schema::ResponseResult::PaneLinkActivated { handled: true, .. }) => {
                self.url_click_consumes_until_up = completed_before_release;
                (false, Vec::new())
            }
            Ok(crate::api::schema::ResponseResult::PaneLinkActivated {
                url: Some(url),
                handled: false,
            }) if crate::app::actions::safe_web_url(&url).is_some() => {
                self.url_click_consumes_until_up = completed_before_release;
                (false, vec![ClientShellAction::OpenSafeWebUrl(url)])
            }
            Ok(crate::api::schema::ResponseResult::PaneLinkActivated { .. }) => {
                (false, replay_action(replay))
            }
            Ok(_) => {
                tracing::warn!("endpoint returned an unexpected link result");
                (true, replay_action(replay))
            }
            Err(error)
                if matches!(
                    error.code.as_deref(),
                    Some("stale_content" | "stale_target" | "endpoint_cancelled")
                ) =>
            {
                self.url_click_consumes_until_up = completed_before_release;
                (false, Vec::new())
            }
            Err(_) => (true, replay_action(replay)),
        }
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
