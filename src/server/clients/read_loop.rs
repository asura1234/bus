//! Decode and bound client-shell input before forwarding ordered server events.
use super::events::ServerEvent;
use super::handshake::client_shell_geometry_error;
use crate::platform::ipc::LocalStream;
use crate::protocol::wire::{
    self as protocol, ClientMessage, ClientPaneInputEvent, MAX_CLIPBOARD_IMAGE_PAYLOAD,
    MAX_GRAPHICS_FRAME_SIZE,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

#[derive(serde::Deserialize)]
struct EndpointRequestHead {
    id: String,
    method: String,
}

pub(super) enum DecodedEndpointRequest {
    Dispatch(Box<crate::protocol::api::schema::Request>),
    Error {
        request_id: String,
        code: &'static str,
        message: String,
    },
}

pub(super) fn decode_endpoint_request(request: &str) -> serde_json::Result<DecodedEndpointRequest> {
    let head = serde_json::from_str::<EndpointRequestHead>(request)?;
    if !crate::server::clients::requests::supports_client_shell_method_name(&head.method) {
        return Ok(DecodedEndpointRequest::Error {
            request_id: head.id,
            code: "unsupported_method",
            message: format!("method {:?} is not available on this machine", head.method),
        });
    }
    Ok(
        match serde_json::from_str::<crate::protocol::api::schema::Request>(request) {
            Ok(request) => DecodedEndpointRequest::Dispatch(Box::new(request)),
            Err(error) => DecodedEndpointRequest::Error {
                request_id: head.id,
                code: "invalid_request",
                message: format!("invalid endpoint request: {error}"),
            },
        },
    )
}

/// Maximum structured input events accepted in one client message.
pub(super) const MAX_INPUT_EVENT_BATCH: usize = 4096;

/// Maximum text and paste payload size (bytes) in one pane-input message.
pub(super) const MAX_INPUT_PAYLOAD: usize = 1024 * 1024; // 1 MB
#[derive(Debug, PartialEq)]
pub(super) enum InputEventLimit {
    WithinLimits,
    TooManyEvents,
    PasteTooLarge { size: usize },
    InputPayloadTooLarge { size: usize },
}

pub(super) fn pane_input_event_limit(events: &[ClientPaneInputEvent]) -> InputEventLimit {
    let mut expanded_events = 0usize;
    let mut paste_bytes = 0usize;
    let mut input_bytes = 0usize;
    for event in events {
        expanded_events = expanded_events.saturating_add(match event {
            ClientPaneInputEvent::Key { repeat_count, .. } => usize::from((*repeat_count).max(1)),
            ClientPaneInputEvent::Mouse {
                kind:
                    crate::protocol::wire::ClientMouseKind::ScrollUp
                    | crate::protocol::wire::ClientMouseKind::ScrollDown,
                lines,
                ..
            } => usize::from((*lines).max(1)),
            ClientPaneInputEvent::TextCommit(_)
            | ClientPaneInputEvent::Mouse { .. }
            | ClientPaneInputEvent::Paste(_) => 1,
        });
        match event {
            ClientPaneInputEvent::Key {
                repeat_count,
                generated_text,
                ..
            } => {
                if let Some(text) = generated_text {
                    input_bytes = input_bytes.saturating_add(
                        text.len()
                            .saturating_mul(usize::from((*repeat_count).max(1))),
                    );
                }
            }
            ClientPaneInputEvent::TextCommit(text) => {
                input_bytes = input_bytes.saturating_add(text.len());
            }
            ClientPaneInputEvent::Mouse { .. } => {}
            ClientPaneInputEvent::Paste(text) => {
                paste_bytes = paste_bytes.saturating_add(text.len());
            }
        }
    }

    classify_input_event_size(expanded_events, paste_bytes, input_bytes)
}

fn classify_input_event_size(
    expanded_events: usize,
    paste_bytes: usize,
    input_bytes: usize,
) -> InputEventLimit {
    if expanded_events > MAX_INPUT_EVENT_BATCH {
        return InputEventLimit::TooManyEvents;
    }

    let payload_bytes = paste_bytes.saturating_add(input_bytes);
    if payload_bytes <= MAX_INPUT_PAYLOAD {
        InputEventLimit::WithinLimits
    } else if input_bytes == 0 {
        InputEventLimit::PasteTooLarge {
            size: payload_bytes,
        }
    } else {
        InputEventLimit::InputPayloadTooLarge {
            size: payload_bytes,
        }
    }
}

/// The client read loop — reads messages from the client and forwards to the server event channel.
pub(super) fn client_read_loop(
    mut stream: LocalStream,
    client_id: u64,
    server_event_tx: &mpsc::Sender<ServerEvent>,
    should_quit: &Arc<AtomicBool>,
) {
    while !should_quit.load(Ordering::Acquire) {
        let Some(msg) = read_client_message(&mut stream, client_id) else {
            let _ = server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
            break;
        };
        if matches!(msg, ClientMessage::Detach) {
            let _ = server_event_tx.blocking_send(ServerEvent::ClientDetach { client_id });
            break;
        }
        match decode_client_message(msg, client_id) {
            Ok(Some(event)) => {
                if server_event_tx.blocking_send(event).is_err() {
                    break; // Main loop gone.
                }
            }
            Ok(None) => {}
            Err(()) => {
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
                break;
            }
        }
    }
    debug!(target: "bus::server::clients::transport", client_id, "client read thread exiting");
}

fn read_client_message(stream: &mut LocalStream, client_id: u64) -> Option<ClientMessage> {
    match protocol::read_message(stream, MAX_GRAPHICS_FRAME_SIZE) {
        Ok(msg) => Some(msg),
        Err(protocol::FramingError::UnexpectedEof) => {
            // Client disconnected.
            None
        }
        Err(protocol::FramingError::Oversized { claimed, max }) => {
            warn!(target: "bus::server::clients::transport",
                client_id,
                claimed, max, "oversized message from client, closing"
            );
            None
        }
        Err(err) => {
            debug!(target: "bus::server::clients::transport", client_id, err = %err, "client read error, closing");
            None
        }
    }
}

fn decode_client_message(msg: ClientMessage, client_id: u64) -> Result<Option<ServerEvent>, ()> {
    let event = match msg {
        ClientMessage::TerminalHello { .. }
        | ClientMessage::Input { .. }
        | ClientMessage::Resize { .. }
        | ClientMessage::ClientShellHello { .. }
        | ClientMessage::GraphicsTransmissionResult { .. }
        | ClientMessage::GraphicsTransmissionStarted { .. } => {
            warn!(target: "bus::server::clients::transport",
                client_id,
                "unexpected non-shell message from client, closing"
            );
            return Err(());
        }
        ClientMessage::ClipboardImage {
            target,
            extension,
            data,
        } => {
            if data.len() > MAX_CLIPBOARD_IMAGE_PAYLOAD {
                warn!(target: "bus::server::clients::transport",
                    client_id,
                    size = data.len(),
                    "oversized clipboard image from client, closing"
                );
                return Err(());
            } else {
                ServerEvent::ClientClipboardImage {
                    client_id,
                    target,
                    extension,
                    data,
                }
            }
        }
        ClientMessage::ClientShellResize {
            cell_width_px,
            cell_height_px,
            surface_size,
            pixel_mouse,
        } => {
            if let Some(reason) =
                client_shell_geometry_error(surface_size, cell_width_px, cell_height_px)
            {
                warn!(target: "bus::server::clients::transport", client_id, %reason, "invalid client shell resize, closing");
                return Err(());
            }
            ServerEvent::ClientShellResize {
                client_id,
                surface_cols: surface_size.cols,
                surface_rows: surface_size.rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
            }
        }
        ClientMessage::ClientShellHostTheme { update } => {
            if matches!(
                &update,
                crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(colors)
                    if colors.len() > 256
            ) {
                warn!(target: "bus::server::clients::transport", client_id, "invalid client shell host theme update, closing");
                return Err(());
            }
            ServerEvent::ClientShellHostTheme { client_id, update }
        }
        ClientMessage::ClientShellFocus { focused } => {
            ServerEvent::ClientShellFocus { client_id, focused }
        }
        ClientMessage::ClientShellMouseCapture { enabled } => {
            ServerEvent::ClientShellMouseCapture { client_id, enabled }
        }
        ClientMessage::ClientShellPaneInput { pane_id, events } => {
            return decode_pane_input(client_id, pane_id, events).map(Some);
        }
        ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
            return decode_client_endpoint_request(client_id, boot_id, request).map(Some);
        }
        ClientMessage::EndpointControl { kind, data }
            if kind == crate::protocol::wire::handshake::PRESENTATION_EFFECTS_SYNC_KIND =>
        {
            ServerEvent::ClientShellPresentationSync {
                client_id,
                token: data,
            }
        }
        ClientMessage::EndpointControl { kind, .. } => {
            debug!(target: "bus::server::clients::transport", client_id, %kind, "ignoring unknown endpoint control message");
            return Ok(None);
        }
        // Detach is handled by the read loop so it terminates after forwarding.
        ClientMessage::Detach => return Ok(None),
    };
    Ok(Some(event))
}

fn decode_pane_input(
    client_id: u64,
    pane_id: String,
    events: Vec<ClientPaneInputEvent>,
) -> Result<ServerEvent, ()> {
    Ok(match pane_input_event_limit(&events) {
        InputEventLimit::WithinLimits => ServerEvent::ClientShellPaneInput {
            client_id,
            pane_id,
            events,
        },
        InputEventLimit::TooManyEvents => {
            warn!(target: "bus::server::clients::transport",
                client_id,
                count = events.len(),
                "oversized targeted pane input batch, closing"
            );
            return Err(());
        }
        InputEventLimit::PasteTooLarge { size } => {
            warn!(target: "bus::server::clients::transport",
                client_id,
                size,
                max = MAX_INPUT_PAYLOAD,
                "oversized targeted pane paste, rejecting"
            );
            ServerEvent::ClientPasteRejected {
                client_id,
                size,
                max: MAX_INPUT_PAYLOAD,
            }
        }
        InputEventLimit::InputPayloadTooLarge { size } => {
            warn!(target: "bus::server::clients::transport",
                client_id,
                size,
                max = MAX_INPUT_PAYLOAD,
                "oversized targeted pane input, closing"
            );
            return Err(());
        }
    })
}

fn decode_client_endpoint_request(
    client_id: u64,
    boot_id: String,
    request: String,
) -> Result<ServerEvent, ()> {
    if boot_id.len() > crate::server::clients::requests::MAX_ENDPOINT_BOOT_ID_BYTES
        || request.len() > crate::server::clients::requests::MAX_ENDPOINT_COMMAND_BYTES
    {
        warn!(target: "bus::server::clients::transport",
            client_id,
            boot_id_size = boot_id.len(),
            request_size = request.len(),
            "oversized client shell endpoint command, closing"
        );
        return Err(());
    }
    let decoded = match decode_endpoint_request(&request) {
        Ok(decoded) => decoded,
        Err(error) => {
            warn!(target: "bus::server::clients::transport", client_id, %error, "invalid endpoint request envelope, closing");
            return Err(());
        }
    };
    let request_id = match &decoded {
        DecodedEndpointRequest::Dispatch(request) => request.id.as_str(),
        DecodedEndpointRequest::Error { request_id, .. } => request_id,
    };
    if request_id.len() > crate::server::clients::requests::MAX_ENDPOINT_REQUEST_ID_BYTES {
        warn!(target: "bus::server::clients::transport",
            client_id,
            "oversized client shell endpoint request id, closing"
        );
        return Err(());
    }
    Ok(match decoded {
        DecodedEndpointRequest::Dispatch(request) => ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id,
            request,
        },
        DecodedEndpointRequest::Error {
            request_id,
            code,
            message,
        } => ServerEvent::ClientShellEndpointRequestError {
            client_id,
            boot_id,
            request_id,
            code,
            message,
        },
    })
}
