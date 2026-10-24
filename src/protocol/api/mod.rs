//! JSON API client, schema, runtime status, and protocol constants.

pub mod client;
pub mod schema;
mod status;
pub(crate) mod submission;

pub(crate) use schema::methods::api_method_name;
pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

/// Definite non-delivery: no Enter was sent. A paste, if queued, was
/// withdrawn before returning this error; the wire code remains compatible.
pub(crate) const AGENT_PROMPT_NOT_SHOWN: &str = "agent_prompt_not_shown";
/// One budget for an agent pane's shell startup to settle and for confirming
/// provider startup. Ordinary busy-shell retries retain their shorter window.
pub(crate) const AGENT_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub(crate) const AGENT_PROMPT_NOT_SHOWN_MESSAGE: &str =
    "Prompt was not submitted: no Enter was sent for this delivery";

pub use crate::utils::env::SOCKET_PATH_ENV_VAR;

pub fn socket_path() -> PathBuf {
    crate::utils::paths::active_api_socket_path()
}
