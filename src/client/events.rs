use crate::protocol::wire::ServerMessage;
use std::io;

/// Internal events for the client event loop.
pub(super) enum ClientLoopEvent {
    #[cfg(unix)]
    StdinInput(Vec<u8>),
    #[cfg(unix)]
    PixelMouse(Vec<u8>, crate::protocol::keys::mouse::HostGeometry),
    #[cfg(windows)]
    StdinEvents(Vec<crate::protocol::wire::ClientInputEvent>),
    Resize(u16, u16, u32, u32, bool),
    TerminalUnavailable(io::Error),
    ServerMessage {
        message: Box<ServerMessage>,
    },
    ServerDisconnected,
    Timer,
}
