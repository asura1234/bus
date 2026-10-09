//! Compatibility facade for blocking client-shell transport.
pub(crate) use super::events::ServerEvent;
pub(crate) use super::handshake::handle_client_handshake;
pub(crate) use super::writer::ClientWriter;

#[cfg(test)]
use super::handshake::{
    client_shell_geometry_error, HANDSHAKE_TIMEOUT, MAX_CLIENT_CELL_SIZE_PX,
    MAX_CLIENT_SHELL_DIMENSION,
};
#[cfg(test)]
use super::read_loop::{
    client_read_loop, decode_endpoint_request, pane_input_event_limit, DecodedEndpointRequest,
    InputEventLimit, MAX_INPUT_EVENT_BATCH, MAX_INPUT_PAYLOAD,
};
#[cfg(test)]
use super::writer::{
    client_writer_loop, ClientControlWriter, ClientRenderWriter, ClientWriterQueue,
};
#[cfg(test)]
use crate::platform::ipc::LocalStream;
#[cfg(test)]
use crate::protocol::wire::handshake::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_PROTOCOL_GENERATION,
    ENDPOINT_WELCOME_KIND,
};
#[cfg(test)]
use crate::protocol::wire::{
    self as protocol, ClientMessage, ClientPaneInputEvent, ServerMessage, MAX_FRAME_SIZE,
};
#[cfg(all(test, not(windows)))]
use interprocess::local_socket::traits::Stream as _;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::mpsc::{SendError, TrySendError};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use tokio::sync::mpsc;

#[cfg(test)]
#[path = "tests/handshake_test.rs"]
mod tests;

#[cfg(test)]
use std::io::Write;
