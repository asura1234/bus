use super::frame::TerminalFrame;
use super::host_theme::ClientHostThemeUpdate;
use super::input::ClientPaneInputEvent;
use super::notifications::{NotifyKind, SemanticNotification};
use super::shell::ClientShellSnapshot;
use super::surface::{
    ClientSurfaceSize, PaneSurfaceFrame, PaneSurfacePatch, RenderEncoding, SurfaceGraphicsAssetKey,
};
use serde::{Deserialize, Serialize};

/// Messages sent from the client to the server over the client protocol socket.
///
/// Client and server are always the same build, so variants may be added, removed
/// or reordered freely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Direct terminal handshake: announces protocol version and terminal dimensions.
    TerminalHello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },

    /// Raw input bytes read from the client's stdin.
    Input {
        /// Raw terminal input (possibly multi-byte escape sequences).
        data: Vec<u8>,
    },

    /// Image bytes read from the client's local clipboard for remote paste bridging.
    ClipboardImage {
        /// Stable terminal target selected by the client that read the clipboard.
        target: ClientClipboardImageTarget,
        /// Image file extension without a leading dot.
        extension: String,
        /// Raw image bytes.
        data: Vec<u8>,
    },

    /// Terminal resize notification from the client.
    Resize {
        /// New terminal width in columns.
        cols: u16,
        /// New terminal height in rows.
        rows: u16,
        /// Width of a terminal cell in physical pixels, or 0 when client-side Kitty graphics are disabled.
        cell_width_px: u32,
        /// Height of a terminal cell in physical pixels, or 0 when unavailable.
        cell_height_px: u32,
        /// Whether this resize carries coherent exact geometry for SGR pixel mouse input.
        pixel_mouse: bool,
    },

    /// Graceful disconnect request.
    Detach,

    /// Result of the one armed Bus-owned direct Kitty transmission.
    GraphicsTransmissionResult {
        transfer_id: u64,
        image_id: u32,
        success: bool,
    },

    /// The direct command was written and flushed; terminal response timing starts now.
    GraphicsTransmissionStarted { transfer_id: u64, image_id: u32 },

    /// Handshake for the client-owned shell around one pane surface.
    ClientShellHello {
        version: u32,
        cell_width_px: u32,
        cell_height_px: u32,
        surface_size: ClientSurfaceSize,
        pixel_mouse: bool,
        direct_graphics: bool,
        /// Whether the endpoint's keymap, rather than the client's, owns shell bindings.
        endpoint_keybindings: bool,
        /// Whether this client wants shell mouse capture even without pane demand.
        mouse_capture: bool,
    },

    /// Resize the pane viewport of a client-owned shell.
    ClientShellResize {
        cell_width_px: u32,
        cell_height_px: u32,
        surface_size: ClientSurfaceSize,
        /// Whether this resize carries coherent exact geometry for SGR pixel mouse input.
        pixel_mouse: bool,
    },

    /// Deliver client-classified semantic input directly to a stable pane target.
    ClientShellPaneInput {
        pane_id: String,
        events: Vec<ClientPaneInputEvent>,
    },

    /// Invoke one endpoint operation through this client shell's selected connection.
    ClientShellEndpointRequest { boot_id: String, request: String },

    /// Publish one host terminal color or appearance update observed by a client-owned shell.
    ClientShellHostTheme { update: ClientHostThemeUpdate },

    /// Publish whether the outer terminal containing a client shell has focus.
    ClientShellFocus { focused: bool },

    /// Update this client's shell mouse-capture preference after config reload.
    ClientShellMouseCapture { enabled: bool },

    /// Named JSON control message for the client-owned shell (handshake, snapshots,
    /// presentation sync).
    EndpointControl { kind: String, data: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientClipboardImageTarget {
    Pane(String),
}

// ---------------------------------------------------------------------------
// Server → Client messages
// ---------------------------------------------------------------------------

/// Messages sent from the server to the client over the client protocol socket.
///
/// Client and server are always the same build, so variants may be added, removed
/// or reordered freely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Handshake response: server acknowledges (or rejects) the client.
    Welcome {
        /// Protocol version the server speaks.
        version: u32,
        /// Render encoding selected by the server for this connection.
        encoding: RenderEncoding,
        /// If present, the handshake failed and this describes why.
        /// The client should exit with a clear error message.
        error: Option<String>,
    },

    /// Terminal bytes to write directly for a terminal-ANSI client.
    Terminal(TerminalFrame),

    /// Client-local Kitty graphics bytes to write directly to the host terminal.
    Graphics {
        /// Raw Kitty graphics protocol bytes.
        bytes: Vec<u8>,
    },

    /// Server is shutting down. Clients should exit gracefully.
    ServerShutdown {
        /// Optional reason for the shutdown.
        reason: Option<String>,
    },

    /// A toast notification to be rendered locally by the client.
    Notify {
        /// What kind of notification.
        kind: NotifyKind,
        /// Human-readable title or sound label.
        message: String,
        /// Optional human-readable notification body.
        body: Option<String>,
    },

    /// OSC 52 clipboard data forwarded from a PTY through the server.
    Clipboard {
        /// Base64-encoded clipboard data.
        data: String,
    },

    /// Set the foreground client's outer terminal window title.
    WindowTitle {
        /// Sanitized title to write with OSC 0. `None` restores Bus's default title.
        title: Option<String>,
    },

    /// Client-local runtime config changed on disk; refresh it without reconnecting.
    ReloadSoundConfig,

    /// Whether the client should currently capture host mouse input.
    MouseCapture {
        /// True when Bus mouse UI is enabled or the focused pane app requests mouse reporting.
        enabled: bool,
        /// True only while the focused pane requests DEC SGR pixel mode 1016.
        sgr_pixels: bool,
    },

    /// Ring the foreground client's outer terminal for pane-originated BEL characters.
    TerminalBell {
        /// Number of BEL characters parsed from one PTY read.
        count: u16,
    },

    /// One validated Bus-owned Kitty regular-file RGBA transmission.
    GraphicsFile {
        path: String,
        expected_len: u64,
        image_id: u32,
        transfer_id: u64,
        leading: Vec<u8>,
        control: String,
        /// ClientShell upload identity. `None` targets a direct terminal client.
        surface_asset: Option<SurfaceGraphicsAssetKey>,
    },

    /// Suppress a direct command that expired before terminal delivery.
    GraphicsTransmissionRetired { transfer_id: u64, image_id: u32 },

    /// Initial metadata for a client-owned shell.
    ClientShellSnapshot(Box<ClientShellSnapshot>),

    /// Active-tab pane content rendered at a client-requested origin-relative size.
    PaneSurface(PaneSurfaceFrame),

    /// Ephemeral semantic notification delivered over the private control lane.
    /// It is sent only to currently connected client-rendered shells.
    SemanticNotification(SemanticNotification),

    /// Immediate endpoint error that the client-rendered shell must show regardless of notification policy.
    ClientShellError { message: String },

    /// Whether the focused pane or popup needs the shell host to report every key.
    ClientShellKeyboardReportAll { enabled: bool },

    /// One ordered chunk of the final response to an endpoint operation.
    ClientShellEndpointResponseChunk {
        boot_id: String,
        request_id: String,
        final_chunk: bool,
        data: Vec<u8>,
    },

    /// Incremental terminal-cell update for a previously committed pane surface.
    PaneSurfacePatch(PaneSurfacePatch),

    /// Named JSON control message for the client-owned shell (handshake, snapshots,
    /// presentation sync).
    EndpointControl { kind: String, data: String },
}

// ---------------------------------------------------------------------------
// Color / Modifier conversion helpers
// ---------------------------------------------------------------------------
