//! Read/write one request against an existing private control endpoint.
use super::protocol::{
    encode, poll_write, worker_timeout, ControlledStream, Frame, FrameError, Request, Response,
    IO_TIMEOUT, LEGACY_SOCKET_NAME, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, POLL_INTERVAL,
    RESPONSE_WRITE_TIMEOUT, SOCKET_NAME, WAIT_SOCKET_NAME,
};
use crate::platform::ipc;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

/// Whether a `request` error proves the server never received the complete
/// request line, so the method did not run. Any other error may follow dispatch.
pub(crate) fn undelivered(error: &str) -> bool {
    [
        "invalid_request:",
        "control_unavailable:",
        "request_too_large:",
        "request_timeout:",
        "control_unsent:",
    ]
    .iter()
    .any(|prefix| error.starts_with(prefix))
}

/// One JSON line in each direction, with no endpoint creation or stale-file removal.
pub(crate) fn request(data_dir: &Path, request: &Request) -> Result<Response, String> {
    request_with_timeout(
        data_dir,
        request,
        worker_timeout(&request.method) + RESPONSE_WRITE_TIMEOUT + IO_TIMEOUT * 2,
    )
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
    let stream = ipc::connect_local_stream_timeout(
        &socket_path(data_dir, request),
        remaining.min(IO_TIMEOUT),
    )
    .map_err(|e| format!("control_unavailable: connect to an existing Bus instance: {e}"))?;
    let mut stream = ControlledStream(stream);
    let mut offset = 0;
    let write_deadline = deadline.min(Instant::now() + IO_TIMEOUT);
    while !poll_write(&mut stream.0, &bytes, &mut offset)
        .map_err(|e| format!("control_unsent: {e}"))?
    {
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
                log_read_failure(request, frame.received_bytes(), "response_too_large");
                return Err("response_too_large: response exceeds 16 MiB".into());
            }
            Err(FrameError::Io(error)) => {
                let received = frame.received_bytes();
                let code = if received > 0 {
                    "response_incomplete"
                } else {
                    "control_io"
                };
                log_read_failure(request, received, code);
                return Err(format!(
                    "{code}: response connection closed or failed after {received} bytes ({:?})",
                    error.kind()
                ));
            }
        }
        if Instant::now() >= deadline {
            log_read_failure(request, frame.received_bytes(), "response_timeout");
            return Err("response_timeout: response deadline elapsed".into());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn log_read_failure(request: &Request, received_bytes: usize, reason: &str) {
    tracing::warn!(
        event = "bus.control.response.failed",
        control_request_id = %request.id.chars().take(128).collect::<String>(),
        method = crate::messaging::perf::control_method_label(&request.method),
        stage = "read", reason, received_bytes,
        max_bytes = MAX_RESPONSE_BYTES,
        "Control response could not be read"
    );
}

/// Route all status reads to their own finite connection pool. Interactive
/// calls retain the legacy name fallback: a Bus started by an older build still
/// listens on the legacy name until restarted, and agents inside it keep
/// using newer CLI builds meanwhile, so fall back to that name when only it
/// exists.
fn socket_path(data_dir: &Path, request: &Request) -> std::path::PathBuf {
    if request.method == "message.status" {
        return data_dir.join(WAIT_SOCKET_NAME);
    }
    let current = data_dir.join(SOCKET_NAME);
    let legacy = data_dir.join(LEGACY_SOCKET_NAME);
    if current.symlink_metadata().is_err() && legacy.symlink_metadata().is_ok() {
        legacy
    } else {
        current
    }
}
