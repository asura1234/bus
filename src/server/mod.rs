pub(crate) mod api;
pub(crate) mod app;
mod app_loop;
mod app_queries;
pub(crate) mod app_settings;
pub(crate) mod app_state;
pub(crate) mod clients;
mod config_reload;
pub mod main_loop;
pub(crate) mod notifications;
pub(crate) mod persistence;
#[cfg(test)]
#[path = "tests/render_scale_test.rs"]
mod render_scale_benchmark;
pub(crate) mod rendering;
pub(crate) mod shutdown;
pub(crate) mod socket_paths;
pub(crate) mod startup;
pub(crate) mod terminals;
pub(crate) mod workspaces;
#[cfg(test)]
mod tests {
    use super::clients::input::apply_client_pane_input_events;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    use interprocess::local_socket::traits::Listener as _;
    #[cfg(unix)]
    use interprocess::local_socket::ListenerNonblockingMode;
    #[cfg(unix)]
    use ratatui::layout::Rect;
    use tokio::sync::mpsc;

    use super::clients::connection::{ClientConnection, DeferredRender};
    use super::clients::transport::{ClientWriter, ServerEvent};
    #[cfg(windows)]
    use super::main_loop::spawn_windows_client_accept_thread;
    use super::main_loop::HeadlessServer;
    use crate::platform::ipc::{bind_local_listener, socket_file_identity};
    use crate::protocol::api;
    use crate::protocol::wire::{self as protocol, ServerMessage, MAX_FRAME_SIZE};
    use crate::terminal::events::TerminalEvent;
    use crate::utils::config;
    use bytes::Bytes;

    use crate::protocol::wire::FrameData;

    #[path = "support_test.rs"]
    mod support;
    use support::*;

    #[path = "lifecycle_test.rs"]
    mod lifecycle_tests;

    #[path = "window_title_test.rs"]
    mod window_title_tests;

    #[path = "client_shell_test.rs"]
    mod client_shell_tests;

    #[path = "client_shell_input_test.rs"]
    mod client_shell_input_tests;

    #[path = "surface_lease_test.rs"]
    mod surface_interest_tests;

    include!("tests/incremental_test.rs");
    include!("tests/views_test.rs");
    include!("tests/input_test.rs");
    include!("tests/notifications_test.rs");
}
