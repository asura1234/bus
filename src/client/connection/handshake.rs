use std::io;
#[cfg(unix)]
use std::io::IsTerminal as _;
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
#[cfg(windows)]
use tracing::debug;
use tracing::info;

use crate::ipc::LocalStream;
use crate::protocol::endpoint::{
    EndpointClientHello, EndpointServerWelcome, BLOB_CODEC_V1, ENDPOINT_HELLO_KIND,
    ENDPOINT_PROTOCOL_GENERATION, ENDPOINT_WELCOME_KIND, INPUT_CODEC_V1, SNAPSHOT_CODEC_V1,
    SURFACE_CODEC_V1,
};
use crate::protocol::{self, ClientMessage, ServerMessage, MAX_FRAME_SIZE};

#[cfg(unix)]
use crate::client::host_terminal::setup::is_ssh_session;
use crate::client::ClientError;

/// Time to wait for the server's Welcome reply during the handshake.
pub(in crate::client) const LOCAL_HANDSHAKE_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(any(unix, test))]
pub(in crate::client) fn direct_graphics_profile_values(
    term_program: &str,
    term: &str,
    kitty_window: bool,
    blocked_transport: bool,
    terminals: bool,
) -> bool {
    let supported = term_program.eq_ignore_ascii_case("ghostty")
        || term_program.eq_ignore_ascii_case("wezterm")
        || matches!(term, "xterm-ghostty" | "xterm-kitty" | "xterm-wezterm")
        || kitty_window;
    supported && !blocked_transport && terminals
}

#[cfg(unix)]
fn direct_graphics_profile_allowed() -> bool {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    direct_graphics_profile_values(
        &term_program,
        &term,
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
        is_ssh_session() || std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some(),
        io::stdin().is_terminal() && io::stdout().is_terminal(),
    )
}

#[cfg(not(unix))]
fn direct_graphics_profile_allowed() -> bool {
    false
}

#[cfg(windows)]
fn set_handshake_recv_timeout(
    stream: &LocalStream,
    timeout: Option<Duration>,
    context: &'static str,
) -> Result<(), ClientError> {
    match stream.set_recv_timeout(timeout) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::Unsupported => {
            debug!(err = %err, context, "client socket receive timeout unavailable");
            Ok(())
        }
        Err(err) => Err(ClientError::ConnectionFailed(err)),
    }
}

#[cfg(not(windows))]
fn set_handshake_recv_timeout(
    stream: &LocalStream,
    timeout: Option<Duration>,
    _context: &'static str,
) -> Result<(), ClientError> {
    stream
        .set_recv_timeout(timeout)
        .map_err(ClientError::ConnectionFailed)
}

#[derive(Debug)]
pub(in crate::client) struct HandshakeResult;

/// Performs the client→server handshake.
///
/// The hello carries this build's version (`client_version`). The welcome must
/// name the same build; a different server build is rejected.
pub(in crate::client) fn do_handshake(
    stream: &mut LocalStream,
    cell_width_px: u32,
    cell_height_px: u32,
    exact_cell_size: bool,
    shell_surface_size: crate::protocol::ClientSurfaceSize,
    endpoint_keybindings: bool,
    mouse_capture: bool,
) -> Result<HandshakeResult, ClientError> {
    stream
        .set_nonblocking(false)
        .map_err(ClientError::ConnectionFailed)?;

    let client_build = crate::build_info::version();
    let hello = EndpointClientHello {
        generation: ENDPOINT_PROTOCOL_GENERATION,
        client_version: client_build.clone(),
        cell_width_px,
        cell_height_px,
        surface_size: shell_surface_size,
        pixel_mouse: exact_cell_size && cfg!(unix),
        direct_graphics: exact_cell_size
            && cell_width_px > 0
            && cell_height_px > 0
            && direct_graphics_profile_allowed(),
        endpoint_keybindings,
        mouse_capture,
        surface_active: true,
        snapshot_codecs: vec![SNAPSHOT_CODEC_V1.into()],
        surface_codecs: vec![SURFACE_CODEC_V1.into()],
        input_codecs: vec![INPUT_CODEC_V1.into()],
        blob_codecs: vec![BLOB_CODEC_V1.into()],
    };
    let hello = ClientMessage::EndpointControl {
        kind: ENDPOINT_HELLO_KIND.into(),
        data: serde_json::to_string(&hello).map_err(|error| {
            ClientError::ConnectionFailed(io::Error::new(io::ErrorKind::InvalidData, error))
        })?,
    };
    protocol::write_message(stream, &hello)
        .map_err(|e| ClientError::ConnectionFailed(io::Error::other(e.to_string())))?;

    set_handshake_recv_timeout(
        stream,
        Some(LOCAL_HANDSHAKE_READ_TIMEOUT),
        "client handshake read timeout unavailable",
    )?;
    let welcome: ServerMessage = protocol::read_message(stream, MAX_FRAME_SIZE)?;
    set_handshake_recv_timeout(
        stream,
        None,
        "failed to clear client handshake read timeout",
    )?;

    let ServerMessage::EndpointControl { kind, data } = welcome else {
        return Err(ClientError::Protocol(protocol::FramingError::Io(
            io::Error::new(io::ErrorKind::InvalidData, "expected endpoint welcome"),
        )));
    };
    if kind != ENDPOINT_WELCOME_KIND {
        return Err(ClientError::Protocol(protocol::FramingError::Io(
            io::Error::new(io::ErrorKind::InvalidData, "expected endpoint welcome"),
        )));
    }
    let welcome: EndpointServerWelcome = serde_json::from_str(&data).map_err(|error| {
        ClientError::Protocol(protocol::FramingError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid endpoint welcome: {error}"),
        )))
    })?;
    if let Some(error) = welcome.error {
        return Err(ClientError::HandshakeRejected {
            version: welcome.generation,
            error: error.message,
        });
    }
    if welcome.server_version != client_build {
        return Err(ClientError::HandshakeRejected {
            version: welcome.generation,
            error: format!(
                "server build {} is not this client build {client_build}",
                welcome.server_version
            ),
        });
    }
    if welcome.generation != ENDPOINT_PROTOCOL_GENERATION
        || welcome.snapshot_codec != SNAPSHOT_CODEC_V1
        || welcome.surface_codec != SURFACE_CODEC_V1
        || welcome.input_codec != INPUT_CODEC_V1
        || welcome.blob_codec != BLOB_CODEC_V1
    {
        return Err(ClientError::HandshakeRejected {
            version: welcome.generation,
            error: "server has no compatible endpoint core; update this machine".into(),
        });
    }
    info!(
        generation = welcome.generation,
        server_version = %welcome.server_version,
        "endpoint handshake succeeded"
    );
    Ok(HandshakeResult)
}
