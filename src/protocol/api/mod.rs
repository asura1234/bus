//! JSON API client, schema, runtime status, and protocol constants.

pub mod client;
pub mod schema;
mod status;

pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

pub const SOCKET_PATH_ENV_VAR: &str = "HERDR_SOCKET_PATH";

pub fn socket_path() -> PathBuf {
    crate::utils::paths::active_api_socket_path()
}
