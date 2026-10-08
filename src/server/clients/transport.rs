//! Blocking client socket transport for the headless server.
//!
//! This module owns the client-shell handshake, read loop, and writer loop.
//! It converts socket I/O into [`ServerEvent`] values consumed by
//! `HeadlessServer`.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SendError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::TryClone as _;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::ipc::LocalStream;
use crate::protocol::endpoint::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_PROTOCOL_GENERATION,
    ENDPOINT_WELCOME_KIND,
};
use crate::protocol::{
    self, ClientMessage, ClientPaneInputEvent, ServerMessage, MAX_CLIPBOARD_IMAGE_PAYLOAD,
    MAX_FRAME_SIZE, MAX_GRAPHICS_FRAME_SIZE,
};

/// How long to wait for a client handshake before closing the connection.
/// Set to 4 seconds (rather than 5) to guarantee the connection is closed
/// within the 5-second deadline, even with OS timer slack, thread scheduling,
/// and cleanup overhead.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(4);

/// Maximum text and paste payload size (bytes) in one pane-input message.
const MAX_INPUT_PAYLOAD: usize = 1024 * 1024; // 1 MB
const MAX_CLIENT_SHELL_DIMENSION: u16 = 4096;
const MAX_CLIENT_SHELL_CELLS: u32 = 1_000_000;
const MAX_CLIENT_CELL_SIZE_PX: u32 = 4096;

fn client_shell_geometry_error(
    surface_size: crate::protocol::ClientSurfaceSize,
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

#[derive(serde::Deserialize)]
struct EndpointRequestHead {
    id: String,
    method: String,
}

enum DecodedEndpointRequest {
    Dispatch(Box<crate::api::schema::Request>),
    Error {
        request_id: String,
        code: &'static str,
        message: String,
    },
}

fn write_endpoint_rejection(stream: &mut LocalStream, code: &str, message: impl Into<String>) {
    let welcome = EndpointServerWelcome::incompatible(code, message);
    let response = ServerMessage::EndpointControl {
        kind: ENDPOINT_WELCOME_KIND.into(),
        data: serde_json::to_string(&welcome).unwrap_or_else(|_| "{}".into()),
    };
    let _ = protocol::write_message(stream, &response);
}

fn decode_endpoint_request(request: &str) -> serde_json::Result<DecodedEndpointRequest> {
    let head = serde_json::from_str::<EndpointRequestHead>(request)?;
    if !crate::server::clients::requests::supports_client_shell_method_name(&head.method) {
        return Ok(DecodedEndpointRequest::Error {
            request_id: head.id,
            code: "unsupported_method",
            message: format!("method {:?} is not available on this machine", head.method),
        });
    }
    Ok(
        match serde_json::from_str::<crate::api::schema::Request>(request) {
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
const MAX_INPUT_EVENT_BATCH: usize = 4096;

/// Channels owned by the server side of a client writer thread.
#[derive(Clone, Debug)]
pub(crate) struct ClientWriter {
    /// Reliable control messages such as shutdown, notifications, and clipboard writes.
    pub(crate) control: ClientControlWriter,
    /// Droppable render messages. Capacity is one so slow clients cannot build lag.
    pub(crate) render: ClientRenderWriter,
}

impl ClientWriter {
    /// Drops render-lane work that has not yet been claimed by the writer.
    pub(crate) fn discard_pending_render(&self) {
        self.render.queue.discard_pending_render();
    }

    #[cfg(test)]
    pub(crate) fn test_close(&self) {
        self.render.queue.close_writer();
    }

    #[cfg(test)]
    pub(crate) fn test_channel(
        control: std::sync::mpsc::Sender<Vec<u8>>,
        render: std::sync::mpsc::SyncSender<Vec<u8>>,
    ) -> Self {
        let queue = ClientWriterQueue::new();
        let drain = queue.clone();
        let control_writer = ClientControlWriter::queue(queue.clone());
        let mut render_writer = ClientRenderWriter::queue(queue);
        render_writer.test_render = Some(render.clone());
        let writer = Self {
            control: control_writer,
            render: render_writer,
        };
        std::thread::spawn(move || {
            while let Some(item) = drain.recv() {
                let sent = match item {
                    ClientWriteItem::Control(data) => control.send(data).is_ok(),
                    ClientWriteItem::Render(data) => render.send(data).is_ok(),
                };
                if !sent {
                    break;
                }
            }
            drain.close_writer();
        });
        writer
    }
}

#[derive(Debug)]
pub(crate) struct ClientControlWriter {
    queue: Arc<ClientWriterQueue>,
}

#[derive(Debug)]
pub(crate) struct ClientRenderWriter {
    queue: Arc<ClientWriterQueue>,
    #[cfg(test)]
    test_render: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

macro_rules! writer_handle {
    ($type:ty $(, $test_field:ident)?) => {
        impl Clone for $type {
            fn clone(&self) -> Self {
                self.queue.add_sender();
                Self {
                    queue: self.queue.clone(),
                    $(
                        #[cfg(test)]
                        $test_field: self.$test_field.clone(),
                    )?
                }
            }
        }
        impl Drop for $type {
            fn drop(&mut self) {
                self.queue.remove_sender();
            }
        }
    };
}
writer_handle!(ClientControlWriter);
writer_handle!(ClientRenderWriter, test_render);

impl ClientControlWriter {
    fn queue(queue: Arc<ClientWriterQueue>) -> Self {
        queue.add_sender();
        Self { queue }
    }

    pub(crate) fn send(&self, data: Vec<u8>) -> Result<(), SendError<Vec<u8>>> {
        self.queue.send_control(data)
    }
}

impl ClientRenderWriter {
    fn queue(queue: Arc<ClientWriterQueue>) -> Self {
        queue.add_sender();
        Self {
            queue,
            #[cfg(test)]
            test_render: None,
        }
    }

    pub(crate) fn try_send(&self, data: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        #[cfg(test)]
        if let Some(sender) = &self.test_render {
            return sender.try_send(data);
        }
        self.queue.try_send_render(data)
    }
}

#[derive(Debug)]
struct ClientWriterQueue {
    state: Mutex<ClientWriterQueueState>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct ClientWriterQueueState {
    control: VecDeque<Vec<u8>>,
    render: Option<Vec<u8>>,
    senders: usize,
    writer_alive: bool,
}

enum ClientWriteItem {
    Control(Vec<u8>),
    Render(Vec<u8>),
}

impl ClientWriterQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ClientWriterQueueState {
                writer_alive: true,
                ..ClientWriterQueueState::default()
            }),
            ready: Condvar::new(),
        })
    }

    fn add_sender(&self) {
        let mut state = self.lock_state();
        state.senders = state.senders.saturating_add(1);
    }

    fn remove_sender(&self) {
        let mut state = self.lock_state();
        state.senders = state.senders.saturating_sub(1);
        self.ready.notify_one();
    }

    fn send_control(&self, data: Vec<u8>) -> Result<(), SendError<Vec<u8>>> {
        let mut state = self.lock_state();
        if !state.writer_alive {
            return Err(SendError(data));
        }
        state.control.push_back(data);
        self.ready.notify_one();
        Ok(())
    }

    fn try_send_render(&self, data: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        let mut state = self.lock_state();
        if !state.writer_alive {
            return Err(TrySendError::Disconnected(data));
        }
        if state.render.is_some() {
            return Err(TrySendError::Full(data));
        }
        state.render = Some(data);
        self.ready.notify_one();
        Ok(())
    }

    fn discard_pending_render(&self) {
        let mut state = self.lock_state();
        state.render = None;
        self.ready.notify_all();
    }

    fn recv(&self) -> Option<ClientWriteItem> {
        let mut state = self.lock_state();
        loop {
            if let Some(data) = state.control.pop_front() {
                return Some(ClientWriteItem::Control(data));
            }
            if let Some(data) = state.render.take() {
                return Some(ClientWriteItem::Render(data));
            }
            if state.senders == 0 {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn close_writer(&self) {
        let mut state = self.lock_state();
        state.writer_alive = false;
        state.render = None;
        self.ready.notify_all();
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ClientWriterQueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Internal event sent from client transport threads to the main event loop.
#[derive(Debug)]
pub(crate) enum ServerEvent {
    /// A client-owned shell completed its dedicated handshake.
    ClientShellConnected {
        client_id: u64,
        surface_cols: u16,
        surface_rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
        direct_graphics: bool,
        endpoint_keybindings: bool,
        mouse_capture: bool,
        surface_active: bool,
        writer: ClientWriter,
    },
    /// A fully decoded interactive paste exceeded the text-input limit.
    ClientPasteRejected {
        client_id: u64,
        size: usize,
        max: usize,
    },
    /// A client sent local clipboard image bytes to paste into a remote pane.
    ClientClipboardImage {
        client_id: u64,
        target: crate::protocol::ClientClipboardImageTarget,
        extension: String,
        data: Vec<u8>,
    },
    /// A client-owned shell recomputed its pane viewport.
    ClientShellResize {
        client_id: u64,
        surface_cols: u16,
        surface_rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    /// A client-owned shell delivered semantic input to one stable pane target.
    ClientShellPaneInput {
        client_id: u64,
        pane_id: String,
        events: Vec<ClientPaneInputEvent>,
    },
    /// A client-owned shell published one host terminal theme observation.
    ClientShellHostTheme {
        client_id: u64,
        update: crate::protocol::ClientHostThemeUpdate,
    },
    /// A client-owned shell reported whether its outer terminal has focus.
    ClientShellFocus { client_id: u64, focused: bool },
    /// A client-owned shell updated its local mouse-capture preference.
    ClientShellMouseCapture { client_id: u64, enabled: bool },
    /// The committed shell asks the server to replay presentation effects before input resumes.
    ClientShellPresentationSync { client_id: u64, token: String },
    /// A client-owned shell invoked one endpoint operation through this connection.
    ClientShellEndpointRequest {
        client_id: u64,
        boot_id: String,
        request: Box<crate::api::schema::Request>,
    },
    /// A well-framed endpoint request could not be dispatched by this server.
    ClientShellEndpointRequestError {
        client_id: u64,
        boot_id: String,
        request_id: String,
        code: &'static str,
        message: String,
    },
    /// One chunk of a deferred endpoint operation's final response is ready.
    ClientShellEndpointResponseChunkReady {
        client_id: u64,
        boot_id: String,
        request_id: String,
        final_chunk: bool,
        data: Vec<u8>,
    },
    /// A client detached gracefully.
    ClientDetach { client_id: u64 },
    /// A client connection was lost.
    ClientDisconnected { client_id: u64 },
    /// A client writer drained its render slot and can accept another render.
    ClientWriterDrained { client_id: u64 },
    /// Ctrl+C or external shutdown signal received.
    QuitSignal,
}

#[derive(Debug, PartialEq)]
enum InputEventLimit {
    WithinLimits,
    TooManyEvents,
    PasteTooLarge { size: usize },
    InputPayloadTooLarge { size: usize },
}

fn pane_input_event_limit(events: &[ClientPaneInputEvent]) -> InputEventLimit {
    let mut expanded_events = 0usize;
    let mut paste_bytes = 0usize;
    let mut input_bytes = 0usize;
    for event in events {
        expanded_events = expanded_events.saturating_add(match event {
            ClientPaneInputEvent::Key { repeat_count, .. } => usize::from((*repeat_count).max(1)),
            ClientPaneInputEvent::Mouse {
                kind:
                    crate::protocol::ClientMouseKind::ScrollUp
                    | crate::protocol::ClientMouseKind::ScrollDown,
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
            debug!(client_id, err = %err, context, "client socket receive timeout unavailable");
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
            debug!(client_id, "client disconnected before handshake");
            return Ok(());
        }
        Err(protocol::FramingError::Oversized { claimed, max }) => {
            warn!(client_id, claimed, max, "oversized handshake from client");
            return Ok(());
        }
        Err(err) => {
            debug!(client_id, err = %err, "failed to read client hello");
            return Ok(());
        }
    };
    let ClientMessage::EndpointControl { kind, data } = hello else {
        write_endpoint_rejection(
            &mut stream,
            "invalid_hello",
            "expected endpoint hello as first message",
        );
        return Ok(());
    };
    if kind != ENDPOINT_HELLO_KIND {
        write_endpoint_rejection(
            &mut stream,
            "invalid_hello",
            "expected endpoint hello as first message",
        );
        return Ok(());
    }
    let hello: EndpointClientHello = match serde_json::from_str(&data) {
        Ok(hello) => hello,
        Err(error) => {
            write_endpoint_rejection(
                &mut stream,
                "invalid_hello",
                format!("invalid endpoint hello: {error}"),
            );
            return Ok(());
        }
    };
    let server_version = crate::build_info::version();
    if hello.client_version != server_version {
        write_endpoint_rejection(
            &mut stream,
            "build_mismatch",
            format!("client build {:?} does not match server build {server_version:?}; restart both from the same Bus build", hello.client_version),
        );
        return Ok(());
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
        write_endpoint_rejection(&mut stream, code, reason);
        return Ok(());
    }
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

/// The client writer loop — prioritizes control messages over render frames.
fn client_writer_loop(
    mut stream: LocalStream,
    client_id: u64,
    writer_queue: Arc<ClientWriterQueue>,
    server_event_tx: mpsc::Sender<ServerEvent>,
) {
    while let Some(item) = writer_queue.recv() {
        let written = match item {
            ClientWriteItem::Control(data) => write_framed_bytes(&mut stream, &data),
            ClientWriteItem::Render(data) => {
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientWriterDrained { client_id });
                write_framed_bytes(&mut stream, &data)
            }
        };
        if !written {
            let _ = server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
            break;
        }
    }
    writer_queue.close_writer();
    debug!("client writer thread exiting");
}

fn write_framed_bytes(stream: &mut LocalStream, data: &[u8]) -> bool {
    if let Err(err) = stream.write_all(data) {
        debug!(err = %err, "client write failed, closing writer");
        return false;
    }
    if let Err(err) = stream.flush() {
        debug!(err = %err, "client flush failed, closing writer");
        return false;
    }
    true
}

/// The client read loop — reads messages from the client and forwards to the server event channel.
fn client_read_loop(
    mut stream: LocalStream,
    client_id: u64,
    server_event_tx: &mpsc::Sender<ServerEvent>,
    should_quit: &Arc<AtomicBool>,
) {
    while !should_quit.load(Ordering::Acquire) {
        let msg: ClientMessage = match protocol::read_message(&mut stream, MAX_GRAPHICS_FRAME_SIZE)
        {
            Ok(msg) => msg,
            Err(protocol::FramingError::UnexpectedEof) => {
                // Client disconnected.
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
                break;
            }
            Err(protocol::FramingError::Oversized { claimed, max }) => {
                warn!(
                    client_id,
                    claimed, max, "oversized message from client, closing"
                );
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
                break;
            }
            Err(err) => {
                debug!(client_id, err = %err, "client read error, closing");
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
                break;
            }
        };

        let event = match msg {
            ClientMessage::TerminalHello { .. }
            | ClientMessage::Input { .. }
            | ClientMessage::Resize { .. }
            | ClientMessage::ClientShellHello { .. }
            | ClientMessage::GraphicsTransmissionResult { .. }
            | ClientMessage::GraphicsTransmissionStarted { .. } => {
                warn!(
                    client_id,
                    "unexpected non-shell message from client, closing"
                );
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
                break;
            }
            ClientMessage::ClipboardImage {
                target,
                extension,
                data,
            } => {
                if data.len() > MAX_CLIPBOARD_IMAGE_PAYLOAD {
                    warn!(
                        client_id,
                        size = data.len(),
                        "oversized clipboard image from client, closing"
                    );
                    let _ = server_event_tx
                        .blocking_send(ServerEvent::ClientDisconnected { client_id });
                    break;
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
                    warn!(client_id, %reason, "invalid client shell resize, closing");
                    let _ = server_event_tx
                        .blocking_send(ServerEvent::ClientDisconnected { client_id });
                    break;
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
                    crate::protocol::ClientHostThemeUpdate::PaletteColors(colors)
                        if colors.len() > 256
                ) {
                    warn!(client_id, "invalid client shell host theme update, closing");
                    let _ = server_event_tx
                        .blocking_send(ServerEvent::ClientDisconnected { client_id });
                    break;
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
                match pane_input_event_limit(&events) {
                    InputEventLimit::WithinLimits => ServerEvent::ClientShellPaneInput {
                        client_id,
                        pane_id,
                        events,
                    },
                    InputEventLimit::TooManyEvents => {
                        warn!(
                            client_id,
                            count = events.len(),
                            "oversized targeted pane input batch, closing"
                        );
                        let _ = server_event_tx
                            .blocking_send(ServerEvent::ClientDisconnected { client_id });
                        break;
                    }
                    InputEventLimit::PasteTooLarge { size } => {
                        warn!(
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
                        warn!(
                            client_id,
                            size,
                            max = MAX_INPUT_PAYLOAD,
                            "oversized targeted pane input, closing"
                        );
                        let _ = server_event_tx
                            .blocking_send(ServerEvent::ClientDisconnected { client_id });
                        break;
                    }
                }
            }
            ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                if boot_id.len() > crate::server::clients::requests::MAX_ENDPOINT_BOOT_ID_BYTES
                    || request.len() > crate::server::clients::requests::MAX_ENDPOINT_COMMAND_BYTES
                {
                    warn!(
                        client_id,
                        boot_id_size = boot_id.len(),
                        request_size = request.len(),
                        "oversized client shell endpoint command, closing"
                    );
                    let _ = server_event_tx
                        .blocking_send(ServerEvent::ClientDisconnected { client_id });
                    break;
                }
                let decoded = match decode_endpoint_request(&request) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        warn!(client_id, %error, "invalid endpoint request envelope, closing");
                        let _ = server_event_tx
                            .blocking_send(ServerEvent::ClientDisconnected { client_id });
                        break;
                    }
                };
                let request_id = match &decoded {
                    DecodedEndpointRequest::Dispatch(request) => request.id.as_str(),
                    DecodedEndpointRequest::Error { request_id, .. } => request_id,
                };
                if request_id.len()
                    > crate::server::clients::requests::MAX_ENDPOINT_REQUEST_ID_BYTES
                {
                    warn!(
                        client_id,
                        "oversized client shell endpoint request id, closing"
                    );
                    let _ = server_event_tx
                        .blocking_send(ServerEvent::ClientDisconnected { client_id });
                    break;
                }
                match decoded {
                    DecodedEndpointRequest::Dispatch(request) => {
                        ServerEvent::ClientShellEndpointRequest {
                            client_id,
                            boot_id,
                            request,
                        }
                    }
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
                }
            }
            ClientMessage::EndpointControl { kind, data }
                if kind == crate::protocol::endpoint::PRESENTATION_EFFECTS_SYNC_KIND =>
            {
                ServerEvent::ClientShellPresentationSync {
                    client_id,
                    token: data,
                }
            }
            ClientMessage::EndpointControl { kind, .. } => {
                debug!(client_id, %kind, "ignoring unknown endpoint control message");
                continue;
            }
            ClientMessage::Detach => {
                let _ = server_event_tx.blocking_send(ServerEvent::ClientDetach { client_id });
                break;
            }
        };

        if server_event_tx.blocking_send(event).is_err() {
            break; // Main loop gone.
        }
    }

    debug!(client_id, "client read thread exiting");
}

#[cfg(test)]
#[path = "tests/handshake_test.rs"]
mod tests;
