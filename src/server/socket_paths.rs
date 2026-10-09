//! Server policy for the binary client socket; platform owns the mechanics.
use std::io;
use std::path::Path;

/// Socket permission mode (owner read/write only).
const SOCKET_PERMISSION_MODE: u32 = 0o600;

/// Prepares a socket path for binding: creates parent directories,
/// removes stale socket files where no server is listening, and rejects live
/// sockets that are already in use.
pub(crate) fn prepare_socket_path(path: &Path) -> io::Result<()> {
    crate::platform::ipc::prepare_socket_path(path, |path| {
        format!(
            "bus server is already running (socket busy at {})",
            path.display()
        )
    })
}

/// Restricts socket file permissions to owner-only (0o600).
pub(crate) fn restrict_socket_permissions(path: &Path) -> io::Result<()> {
    crate::platform::ipc::restrict_socket_permissions(path, SOCKET_PERMISSION_MODE)
}

#[cfg(all(test, unix))]
#[path = "tests/socket_paths_test.rs"]
mod tests;
