//! Thin client mode — connects to the server's client socket.
//!
//! The client:
//! - Connects to `herdr-client.sock`, sends the shell hello with this build's version
//! - Sets up the real terminal (raw mode, mouse capture, keyboard enhancements)
//! - Receives Frame messages and blits them to the terminal (diff against last frame)
//! - Reads stdin events (keystrokes, mouse, paste) and sends them as ClientMessage::Input
//! - Detects terminal resize and sends ClientMessage::Resize
//! - Restores terminal on exit (normal or error)
//! - Handles ServerShutdown gracefully (clean exit, informative message to stderr)
//! - Handles server unreachable (clear error screen, not blank/hang)
//! - Forwards OSC 52 clipboard writes from server to its own stdout
//! - Displays sound/toast notifications forwarded from server

mod clipboard;
pub(crate) mod compositor;
mod config_reload;
pub(crate) mod connection;
mod effects;
pub(crate) mod errors;
mod event_loop;
mod events;
pub(crate) mod host_terminal;
mod notifications;
pub(crate) mod panes;
mod rooms;
pub(crate) mod run;
mod server_messages;
mod state;
mod timer;

pub use errors::ClientError;
pub use run::run_client;

#[cfg(test)]
#[path = "tests/client_test.rs"]
mod tests;
