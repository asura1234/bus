#[cfg(unix)]
pub(crate) mod accept;
pub(crate) mod clipboard_images;
pub(crate) mod connection;
pub(crate) mod focus;
pub(crate) mod input;
pub(crate) mod requests;
pub(crate) mod surface_lease;
pub(crate) mod transport;

pub(crate) mod events;
pub(crate) mod handshake;
pub(crate) mod read_loop;
pub(crate) mod writer;

mod foreground;
mod shell_request_kinds;
