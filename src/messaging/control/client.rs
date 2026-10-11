//! Read/write one request against an existing private control endpoint.
use super::protocol::{
    encode, poll_write, ControlledStream, Frame, FrameError, Request, Response, IO_TIMEOUT,
    LEGACY_SOCKET_NAME, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, POLL_INTERVAL, SOCKET_NAME,
    WORKER_TIMEOUT,
};
use crate::platform::ipc;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

/// One JSON line in each direction, with no endpoint creation or stale-file removal.
pub(crate) fn request(data_dir: &Path, request: &Request) -> Result<Response, String> {
    request_with_timeout(data_dir, request, WORKER_TIMEOUT + IO_TIMEOUT * 3)
}

pub(crate) fn request_with_timeout(
    data_dir: &Path,
    request: &Request,
    timeout: Duration,
) -> Result<Response, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("invalid_request: timeout is too large")?;
    if request.id.is_empty() || request.method.is_empty() {
        return Err("invalid_request: id and method must be nonempty".into());
    }
    ipc::validate_private_socket_directory(data_dir).map_err(|e| {
        format!("control_unavailable: connect to a private existing Bus data directory: {e}")
    })?;
    let bytes = encode(request, MAX_REQUEST_BYTES)
        .map_err(|_| "request_too_large: request exceeds 256 KiB".to_string())?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("request_timeout: request deadline elapsed".into());
    }
    let stream =
        ipc::connect_local_stream_timeout(&socket_path(data_dir), remaining.min(IO_TIMEOUT))
            .map_err(|e| {
                format!("control_unavailable: connect to an existing Bus instance: {e}")
            })?;
    let mut stream = ControlledStream(stream);
    let mut offset = 0;
    let write_deadline = deadline.min(Instant::now() + IO_TIMEOUT);
    while !poll_write(&mut stream.0, &bytes, &mut offset).map_err(|e| format!("control_io: {e}"))? {
        if Instant::now() >= write_deadline {
            return Err("request_timeout: request write deadline elapsed".into());
        }
        thread::sleep(POLL_INTERVAL);
    }
    let mut frame = Frame::default();
    loop {
        match frame.poll(&mut stream.0, MAX_RESPONSE_BYTES) {
            Ok(Some(bytes)) => {
                let response: Response = serde_json::from_slice(&bytes).map_err(|_| {
                    "invalid_response: response is not a valid control envelope".to_string()
                })?;
                let anonymous_error = response.id.is_empty() && !response.ok;
                if (response.id != request.id && !anonymous_error)
                    || response.ok == response.error.is_some()
                {
                    return Err("invalid_response: response identity or outcome is invalid".into());
                }
                return Ok(response);
            }
            Ok(None) => {}
            Err(FrameError::TooLarge) => {
                return Err("response_too_large: response exceeds 16 MiB".into())
            }
            Err(FrameError::Io(_)) => {
                return Err("control_io: response connection closed or failed".into())
            }
        }
        if Instant::now() >= deadline {
            return Err("response_timeout: response deadline elapsed".into());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// The session's control socket. A Bus started by an older build still
/// listens on the legacy name until restarted, and agents inside it keep
/// using newer CLI builds meanwhile, so fall back to that name when only it
/// exists.
fn socket_path(data_dir: &Path) -> std::path::PathBuf {
    let current = data_dir.join(SOCKET_NAME);
    let legacy = data_dir.join(LEGACY_SOCKET_NAME);
    if current.symlink_metadata().is_err() && legacy.symlink_metadata().is_ok() {
        legacy
    } else {
        current
    }
}
