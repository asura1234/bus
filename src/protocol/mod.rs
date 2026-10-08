//! Shared wire protocol and presentation encoding code.

pub(crate) mod ansi;
pub mod api;
pub(crate) mod keys;
pub(crate) mod kitty;
pub(crate) mod wire;

pub(crate) use ansi as render_ansi;
pub use wire::handshake as endpoint;
pub use wire::*;
