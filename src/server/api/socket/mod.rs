//! API socket lifecycle facade and stable dispatch helpers.
mod accept;
mod connection;

use crate::platform::ipc::{remove_socket_file_if_owned, SocketFileIdentity};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

pub(crate) use accept::start_server_with_stop_control;
pub(crate) use connection::api_method_name;
pub(in crate::server::api) use connection::{
    dispatch_to_app_with_caller_timeout, dispatch_to_app_with_timeout, should_stop_connection,
};

pub(super) const CONNECTION_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const APP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ServerHandle {
    _thread: std::thread::JoinHandle<()>,
    path: PathBuf,
    identity: SocketFileIdentity,
    running: Arc<AtomicBool>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);

        if let Err(err) = self.remove_socket_file_if_owned() {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %self.path.display(), err = %err, "failed to remove api socket on shutdown");
            }
        }
    }
}

impl ServerHandle {
    pub(crate) fn remove_socket_file_if_owned(&self) -> std::io::Result<()> {
        remove_socket_file_if_owned(&self.path, &self.identity)
    }
}

#[cfg(test)]
use crate::platform::ipc::LocalStream;
#[cfg(test)]
use crate::protocol::api::schema::{ErrorResponse, Method, Request};
#[cfg(all(test, unix))]
use crate::protocol::api::schema::{ResponseResult, SuccessResponse};
#[cfg(test)]
use crate::server::api::EventHub;
#[cfg(all(test, unix))]
use crate::server::api::{ApiRequestMessage, ApiRequestSender};
#[cfg(all(test, unix))]
use accept::{default_capabilities, restrict_socket_permissions, SOCKET_PERMISSION_MODE};
#[cfg(all(test, unix))]
use connection::api_response_outcome;
#[cfg(test)]
use connection::handle_connection;
#[cfg(all(test, windows))]
use connection::{read_initial_request_line_with_limits, read_initial_request_line_with_timeout};
#[cfg(all(test, unix))]
use std::fs;
#[cfg(all(test, windows))]
use std::io;
#[cfg(test)]
use std::io::Write;

#[cfg(test)]
include!("tests/dispatch_test.rs");

#[cfg(all(test, windows))]
#[path = "tests/connection_test.rs"]
mod windows_tests;

#[cfg(all(test, unix))]
#[path = "tests/accept_test.rs"]
mod tests;

#[cfg(all(test, unix))]
use crate::protocol::api::socket_path;
#[cfg(all(test, windows))]
use std::time::Instant;
