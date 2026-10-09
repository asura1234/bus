//! Bind and secure the API listener, then accept per-connection threads.
use super::connection::handle_connection_with_stop;
use super::ServerHandle;
use crate::platform::ipc::{bind_local_listener, socket_file_identity};
use crate::protocol::api::schema::ServerCapabilities;
use crate::protocol::api::socket_path;
use crate::server::api::{ApiRequestSender, EventHub};
use interprocess::local_socket::traits::ListenerExt as _;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

pub(super) const SOCKET_PERMISSION_MODE: u32 = 0o600;

pub(crate) fn start_server_with_stop_control(
    api_tx: ApiRequestSender,
    event_hub: EventHub,
    server_stop: Arc<AtomicBool>,
) -> std::io::Result<ServerHandle> {
    start_server_inner(api_tx, event_hub, default_capabilities(), server_stop)
}

pub(super) fn default_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        detached_server_daemon: crate::platform::current_process_is_detached_server_daemon(),
        endpoint_protocol_generation: Some(
            crate::protocol::wire::handshake::ENDPOINT_PROTOCOL_GENERATION,
        ),
        surface_interest: true,
        health_check: true,
    }
}

pub(super) fn start_server_inner(
    api_tx: ApiRequestSender,
    event_hub: EventHub,
    capabilities: ServerCapabilities,
    server_stop: Arc<AtomicBool>,
) -> std::io::Result<ServerHandle> {
    let path = socket_path();
    prepare_socket_path(&path)?;

    let listener = bind_local_listener(&path)?;
    restrict_socket_permissions(&path)?;
    let identity = socket_file_identity(&path)?;
    info!(target: "bus::server::api::socket", path = %path.display(), "api server listening");

    let running = Arc::new(AtomicBool::new(true));
    let listener_running = Arc::clone(&running);
    let thread = std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let api_tx = api_tx.clone();
                    let event_hub = event_hub.clone();
                    let capabilities = capabilities.clone();
                    let server_stop = Arc::clone(&server_stop);
                    let connection_running = Arc::clone(&listener_running);
                    std::thread::spawn(move || {
                        if let Err(err) = handle_connection_with_stop(
                            stream,
                            &api_tx,
                            &event_hub,
                            &connection_running,
                            capabilities,
                            &server_stop,
                        ) {
                            warn!(target: "bus::server::api::socket", err = %err, "api connection failed");
                        }
                    });
                }
                Err(err) => {
                    error!(target: "bus::server::api::socket", err = %err, "api listener accept failed");
                    break;
                }
            }
        }
        debug!(target: "bus::server::api::socket", "api server thread exiting");
    });

    Ok(ServerHandle {
        _thread: thread,
        path,
        identity,
        running,
    })
}

pub(super) fn prepare_socket_path(path: &Path) -> std::io::Result<()> {
    crate::platform::ipc::prepare_socket_path(path, |path| {
        format!("bus is already running (socket busy at {})", path.display())
    })
}

pub(super) fn restrict_socket_permissions(path: &Path) -> std::io::Result<()> {
    crate::platform::ipc::restrict_socket_permissions(path, SOCKET_PERMISSION_MODE)
}
