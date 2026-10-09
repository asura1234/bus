//! Transport event values and their server-side dispatch.
use super::writer::ClientWriter;
use crate::protocol::wire::ClientPaneInputEvent;

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
        target: crate::protocol::wire::ClientClipboardImageTarget,
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
        update: crate::protocol::wire::ClientHostThemeUpdate,
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
        request: Box<crate::protocol::api::schema::Request>,
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

mod connection;
mod shell;
use crate::server::main_loop::HeadlessServer;

impl HeadlessServer {
    /// Handles a server event. Returns true if the event requires a re-render.
    pub(in crate::server) fn handle_server_event(&mut self, ev: ServerEvent) -> bool {
        match ev {
            ev @ (ServerEvent::ClientShellConnected { .. }
            | ServerEvent::ClientDetach { .. }
            | ServerEvent::ClientDisconnected { .. }
            | ServerEvent::ClientWriterDrained { .. }
            | ServerEvent::QuitSignal) => self.handle_client_connection_event(ev),
            ev @ (ServerEvent::ClientPasteRejected { .. }
            | ServerEvent::ClientClipboardImage { .. }
            | ServerEvent::ClientShellResize { .. }
            | ServerEvent::ClientShellHostTheme { .. }
            | ServerEvent::ClientShellFocus { .. }
            | ServerEvent::ClientShellMouseCapture { .. }
            | ServerEvent::ClientShellPresentationSync { .. }
            | ServerEvent::ClientShellPaneInput { .. }
            | ServerEvent::ClientShellEndpointRequestError { .. }
            | ServerEvent::ClientShellEndpointRequest { .. }
            | ServerEvent::ClientShellEndpointResponseChunkReady { .. }) => {
                self.handle_client_shell_event(ev)
            }
        }
    }
}
