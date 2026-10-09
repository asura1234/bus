//! JSON API client, schema, runtime status, and protocol constants.

pub mod client;
pub mod schema;
mod status;

pub(crate) use schema::methods::api_method_name;
pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

pub use crate::utils::env::SOCKET_PATH_ENV_VAR;

pub fn socket_path() -> PathBuf {
    crate::utils::paths::active_api_socket_path()
}
