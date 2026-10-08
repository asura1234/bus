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
mod config_reload;
pub(crate) mod connection;
pub(crate) use connection::bootstrap as endpoint;
use connection::requests as endpoint_commands;
mod errors;
mod events;
pub(crate) mod host_terminal;
use connection::handshake;
use host_terminal::frame_output;
use host_terminal::input;
pub(crate) mod compositor;
mod event_loop;
mod notifications;
pub(crate) mod panes;
mod rooms;
mod server_messages;
use compositor as shell;
mod effects;
mod run;
mod state;
use host_terminal::geometry as terminal_geometry;
use host_terminal::setup as terminal_setup;
mod timer;
use connection as transport;

#[cfg(test)]
use clipboard::decode_clipboard_payload;
use clipboard::forward_clipboard;
#[cfg(test)]
use config_reload::reload_local_client_config;
use config_reload::{apply_reload, init_logging};
use effects::*;
use event_loop::run_client_loop;
use events::ClientLoopEvent;
use run::{ClientInputLifecycle, ClientLoopConfig};
use state::ClientState;
use transport::*;

pub use run::run_client;
#[cfg(test)]
pub(crate) use shell::{ClientShellConfig, ClientShellState};

#[cfg(not(windows))]
use terminal_geometry::query_host_terminal_appearance;
#[cfg(test)]
use terminal_geometry::{
    cell_size_fallback, current_terminal_geometry_with, ioctl_cell_size, pack_cell_size,
    resize_report_required, should_query_host_cell_size, write_host_cell_size_query,
    write_host_terminal_appearance_query, write_host_terminal_theme_query,
};
use terminal_geometry::{
    host_cell_size_query_required, initial_terminal_geometry, query_host_cell_size,
    query_host_terminal_theme, resize_poll_loop, should_query_host_terminal_theme,
};
#[cfg(unix)]
use terminal_geometry::{reported_cell_size_from_events, store_reported_cell_size};
#[cfg(unix)]
use terminal_setup::finish_terminal_input;
use terminal_setup::{
    effective_sgr_pixel_mouse, set_mouse_capture, setup_terminal, should_draw_host_cursor,
};
#[cfg(windows)]
use terminal_setup::{
    enable_windows_virtual_terminal_input, is_ssh_session, windows_vti_input_backend_enabled,
};
#[cfg(test)]
use terminal_setup::{
    should_enable_host_color_scheme_reports, windows_virtual_terminal_input_mode,
    write_host_color_scheme_report_mode, write_terminal_restore_postlude,
};

pub use errors::ClientError;
#[cfg(test)]
use frame_output::{clear_received_kitty_graphics, kitty_graphics_image_ids};
use frame_output::{
    contains_kitty_graphics_bytes, record_received_kitty_graphics,
    write_encoded_frame_with_graphics,
};
#[cfg(test)]
use handshake::direct_graphics_profile_values;
use handshake::do_handshake;
#[cfg(test)]
use notifications::handle_notify_with_notifiers;
use notifications::{forward_terminal_bells, handle_notify};

use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::ipc::LocalStream;
use crate::protocol::render_ansi;
#[cfg(test)]
use crate::protocol::NotifyKind;
use crate::protocol::{self, ClientMessage, FrameData, ServerMessage, MAX_GRAPHICS_FRAME_SIZE};
use crate::server::socket_paths::client_socket_path;

#[cfg(test)]
#[path = "tests/client_test.rs"]
mod tests;
