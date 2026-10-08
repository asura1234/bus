// Temporary composition aliases for the staged module moves; removed in S12.
#[cfg(any(windows, test))]
pub(crate) use crate::platform::console_command as noninteractive_process;
pub(crate) use crate::platform::{ipc, sound};
pub(crate) use crate::protocol::keys::host as raw_input;
pub(crate) use crate::protocol::kitty as kitty_graphics;
pub(crate) use crate::utils::config;
pub(crate) use crate::utils::render::{prof as render_prof, signal as render_signal};
pub(crate) use crate::utils::text::{copy_motion as copy_mode, selection};
pub(crate) use crate::utils::theme::color as terminal_theme;
pub(crate) use crate::utils::{home_path, logging, paths as session, version as build_info};

pub(crate) mod api {
    pub use crate::protocol::api::{
        client, read_runtime_status_at, schema, socket_path, RuntimeStatus, SOCKET_PATH_ENV_VAR,
    };
    pub(crate) use crate::server::api::{
        api_method_name, request_changes_ui, start_server_with_stop_control,
    };
    pub use crate::server::api::{ApiRequestMessage, ApiRequestSender, EventHub, ServerHandle};
}

#[path = "input/lease.rs"]
mod input_lease;

pub(crate) mod input {
    pub(crate) use super::input_lease::{InputLeaseKey, InputLeaseTable, RepeatPlan};
    #[cfg(not(windows))]
    pub use crate::protocol::keys::ime_compatible_keyboard_enhancement_flags;
    pub(crate) use crate::protocol::keys::mouse;
    pub use crate::protocol::keys::{
        encode_terminal_key, host_modify_other_keys_mode, parse_terminal_key_sequence,
        KeyboardProtocol, TerminalKey, TextCommit, WindowsKeyRecord,
    };
}
