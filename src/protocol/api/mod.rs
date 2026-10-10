//! JSON API client, schema, runtime status, and protocol constants.

pub mod client;
pub mod schema;
mod status;

pub(crate) use schema::methods::api_method_name;
pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

/// The provider's input box never showed a pasted prompt as typed, so the
/// server sent no Enter and cleared the paste: a definite non-delivery.
pub(crate) const AGENT_PROMPT_NOT_SHOWN: &str = "agent_prompt_not_shown";
pub(crate) const AGENT_PROMPT_NOT_SHOWN_MESSAGE: &str =
    "Provider input box never showed the prompt as typed; it was cleared and not submitted";

pub use crate::utils::env::SOCKET_PATH_ENV_VAR;

pub fn socket_path() -> PathBuf {
    crate::utils::paths::active_api_socket_path()
}
