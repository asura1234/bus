//! Endpoint hello validation and registration of a connected client shell.
use super::events::ServerEvent;
use super::read_loop::client_read_loop;
use super::writer::{
    client_writer_loop, ClientControlWriter, ClientRenderWriter, ClientWriter, ClientWriterQueue,
};
use crate::platform::ipc::LocalStream;
use crate::protocol::wire::handshake::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_PROTOCOL_GENERATION,
    ENDPOINT_WELCOME_KIND,
};
use crate::protocol::wire::{self as protocol, ClientMessage, ServerMessage, MAX_FRAME_SIZE};
use interprocess::local_socket::traits::Stream as _;
use interprocess::TryClone as _;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// How long to wait for a client handshake before closing the connection.
/// Set to 4 seconds (rather than 5) to guarantee the connection is closed
/// within the 5-second deadline, even with OS timer slack, thread scheduling,
/// and cleanup overhead.
pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(4);

pub(super) const MAX_CLIENT_SHELL_DIMENSION: u16 = 4096;
const MAX_CLIENT_SHELL_CELLS: u32 = 1_000_000;
pub(super) const MAX_CLIENT_CELL_SIZE_PX: u32 = 4096;

pub(super) fn client_shell_geometry_error(
    surface_size: crate::protocol::wire::ClientSurfaceSize,
    cell_width_px: u32,
    cell_height_px: u32,
) -> Option<&'static str> {
    if surface_size.cols == 0 || surface_size.rows == 0 {
        return Some("client shell requires a non-empty pane surface");
    }
    if surface_size.cols > MAX_CLIENT_SHELL_DIMENSION
        || surface_size.rows > MAX_CLIENT_SHELL_DIMENSION
        || u32::from(surface_size.cols) * u32::from(surface_size.rows) > MAX_CLIENT_SHELL_CELLS
    {
        return Some("client shell pane surface exceeds the safe geometry limit");
    }
    if cell_width_px > MAX_CLIENT_CELL_SIZE_PX || cell_height_px > MAX_CLIENT_CELL_SIZE_PX {
        return Some("client shell cell pixel size exceeds the safe geometry limit");
    }
    None
}

fn write_endpoint_rejection(stream: &mut LocalStream, code: &str, message: impl Into<String>) {
    let welcome = EndpointServerWelcome::incompatible(code, message);
    let response = ServerMessage::EndpointControl {
        kind: ENDPOINT_WELCOME_KIND.into(),
        data: serde_json::to_string(&welcome).unwrap_or_else(|_| "{}".into()),
    };
    let _ = protocol::write_message(stream, &response);
}
#[cfg(windows)]
fn set_client_recv_timeout(
    stream: &LocalStream,
    timeout: Option<Duration>,
    context: &'static str,
    client_id: u64,
) -> io::Result<()> {
    match stream.set_recv_timeout(timeout) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::Unsupported => {
            debug!(target: "bus::server::clients::transport", client_id, err = %err, context, "client socket receive timeout unavailable");
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(not(windows))]
fn set_client_recv_timeout(
    stream: &LocalStream,
    timeout: Option<Duration>,
    _context: &'static str,
    _client_id: u64,
) -> io::Result<()> {
    stream.set_recv_timeout(timeout)
}

/// Handles the client handshake on a blocking thread.
///
/// Requires an endpoint hello from the same build, sends its welcome, and
/// enters a read loop forwarding messages to the server event channel.
pub(crate) fn handle_client_handshake(
    mut stream: LocalStream,
    client_id: u64,
    server_event_tx: &mpsc::Sender<ServerEvent>,
    should_quit: &Arc<AtomicBool>,
) -> io::Result<()> {
    if should_quit.load(Ordering::Acquire) {
        return Ok(());
    }

    stream.set_nonblocking(false)?;
    set_client_recv_timeout(
        &stream,
        Some(HANDSHAKE_TIMEOUT),
        "client handshake read timeout unavailable",
        client_id,
    )?;

    let hello: ClientMessage = match protocol::read_message(&mut stream, MAX_FRAME_SIZE) {
        Ok(msg) => msg,
        Err(protocol::FramingError::UnexpectedEof) => {
            debug!(target: "bus::server::clients::transport", client_id, "client disconnected before handshake");
            return Ok(());
        }
        Err(protocol::FramingError::Oversized { claimed, max }) => {
            warn!(target: "bus::server::clients::transport", client_id, claimed, max, "oversized handshake from client");
            return Ok(());
        }
        Err(err) => {
            debug!(target: "bus::server::clients::transport", client_id, err = %err, "failed to read client hello");
            return Ok(());
        }
    };
    let Some(hello) = validate_endpoint_hello(&mut stream, hello) else {
        return Ok(());
    };
    if should_quit.load(Ordering::Acquire) {
        return Ok(());
    }

    let welcome = EndpointServerWelcome::compatible(
        crate::server::clients::requests::supported_client_shell_method_names()
            .iter()
            .map(|method| (*method).to_owned())
            .collect(),
    );
    protocol::write_message(
        &mut stream,
        &ServerMessage::EndpointControl {
            kind: ENDPOINT_WELCOME_KIND.into(),
            data: serde_json::to_string(&welcome).map_err(io::Error::other)?,
        },
    )
    .map_err(|e| io::Error::other(e.to_string()))?;
    set_client_recv_timeout(
        &stream,
        None,
        "failed to clear client handshake read timeout",
        client_id,
    )?;

    let writer_queue = ClientWriterQueue::new();
    let writer = ClientWriter {
        control: ClientControlWriter::queue(writer_queue.clone()),
        render: ClientRenderWriter::queue(writer_queue.clone()),
    };
    let write_stream = stream.try_clone()?;
    let writer_event_tx = server_event_tx.clone();
    std::thread::spawn(move || {
        client_writer_loop(write_stream, client_id, writer_queue, writer_event_tx);
    });
    if should_quit.load(Ordering::Acquire) {
        send_shutdown_to_unregistered_client(&writer);
        return Ok(());
    }
    let connected = ServerEvent::ClientShellConnected {
        client_id,
        surface_cols: hello.surface_size.cols,
        surface_rows: hello.surface_size.rows,
        cell_width_px: hello.cell_width_px,
        cell_height_px: hello.cell_height_px,
        pixel_mouse: hello.pixel_mouse,
        direct_graphics: hello.direct_graphics,
        endpoint_keybindings: hello.endpoint_keybindings,
        mouse_capture: hello.mouse_capture,
        surface_active: hello.surface_active,
        writer,
    };
    if let Err(err) = server_event_tx.blocking_send(connected) {
        if let ServerEvent::ClientShellConnected { writer, .. } = err.0 {
            send_shutdown_to_unregistered_client(&writer);
        }
        return Ok(());
    }
    client_read_loop(stream, client_id, server_event_tx, should_quit);
    Ok(())
}

fn send_shutdown_to_unregistered_client(writer: &ClientWriter) {
    let mut framed = Vec::new();
    if protocol::write_message(
        &mut framed,
        &ServerMessage::ServerShutdown {
            reason: Some("server is shutting down".to_owned()),
        },
    )
    .is_ok()
    {
        let _ = writer.control.send(framed);
    }
}

fn validate_endpoint_hello(
    stream: &mut LocalStream,
    hello: ClientMessage,
) -> Option<EndpointClientHello> {
    let ClientMessage::EndpointControl { kind, data } = hello else {
        write_endpoint_rejection(
            stream,
            "invalid_hello",
            "expected endpoint hello as first message",
        );
        return None;
    };
    if kind != ENDPOINT_HELLO_KIND {
        write_endpoint_rejection(
            stream,
            "invalid_hello",
            "expected endpoint hello as first message",
        );
        return None;
    }
    let hello: EndpointClientHello = match serde_json::from_str(&data) {
        Ok(hello) => hello,
        Err(error) => {
            write_endpoint_rejection(
                stream,
                "invalid_hello",
                format!("invalid endpoint hello: {error}"),
            );
            return None;
        }
    };
    let server_version = crate::utils::version::version();
    if hello.client_version != server_version {
        write_endpoint_rejection(
            stream,
            "build_mismatch",
            format!("client build {:?} does not match server build {server_version:?}; restart both from the same Bus build", hello.client_version),
        );
        return None;
    }
    let incompatibility = if hello.generation != ENDPOINT_PROTOCOL_GENERATION {
        Some(("unsupported_generation", format!("endpoint generation {} is unsupported; this server supports generation {ENDPOINT_PROTOCOL_GENERATION}", hello.generation)))
    } else if !hello.supports_required_codecs() {
        Some((
            "no_common_core",
            "client and server have no compatible endpoint core codecs".to_owned(),
        ))
    } else {
        client_shell_geometry_error(
            hello.surface_size,
            hello.cell_width_px,
            hello.cell_height_px,
        )
        .map(|reason| ("invalid_surface", reason.to_owned()))
    };
    if let Some((code, reason)) = incompatibility {
        write_endpoint_rejection(stream, code, reason);
        return None;
    }
    Some(hello)
}
