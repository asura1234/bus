//! Stop the active session server through its existing lifecycle edge.
use super::launch;
use std::io;

pub(super) fn run() -> io::Result<()> {
    let running = launch::is_server_listening();
    if running {
        stop_active_server().map_err(io::Error::other)?;
    }
    super::help::write_stdout_line(format_args!("{}", serde_json::json!({"stopped": running})));
    Ok(())
}

use crate::platform::ipc::LocalStream;
use crate::utils::paths::active_api_socket_path;
use interprocess::local_socket::traits::Stream as _;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) const MIN_SOCKET_TIMEOUT: Duration = Duration::from_millis(1);
pub(crate) const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_WAIT_POLL: Duration = Duration::from_millis(25);
pub(crate) fn stop_active_server() -> Result<(), String> {
    let socket_path = active_api_socket_path();
    let client_socket_path = crate::utils::socket_paths::client_socket_path();
    stop_socket_with_timeout(
        socket_path.clone(),
        vec![socket_path, client_socket_path],
        STOP_WAIT_TIMEOUT,
        "server",
    )
}

pub(crate) fn stop_socket_with_timeout(
    socket_path: PathBuf,
    stopped_socket_paths: Vec<PathBuf>,
    timeout: Duration,
    label: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let request = serde_json::json!({
        "id": "cli:session:stop",
        "method": "server.stop",
        "params": {}
    });
    let stream = crate::platform::ipc::connect_local_stream(&socket_path).map_err(|err| {
        format!(
            "{label} is not running or cannot be reached at {}: {err}",
            socket_path.display()
        )
    })?;
    let stop_response = send_stop_request(stream, &request, deadline)?;
    if let Some(response) = stop_response {
        if let Some(error) = response.get("error") {
            return Err(error.to_string());
        }
    }
    if !wait_until_stopped_until(&stopped_socket_paths, deadline) {
        let reachable = reachable_socket_paths(&stopped_socket_paths);
        return Err(format!(
            "{label} did not stop within {}ms; sockets are still reachable at {}",
            timeout.as_millis(),
            reachable
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

pub(crate) fn send_stop_request(
    mut stream: LocalStream,
    request: &serde_json::Value,
    deadline: Instant,
) -> Result<Option<serde_json::Value>, String> {
    let Some(write_timeout) = socket_timeout_until(deadline) else {
        return Ok(None);
    };
    if let Err(err) = stream.set_send_timeout(Some(write_timeout)) {
        if !stop_timeout_error_allows_wait(&err) {
            return Err(err.to_string());
        }
    }

    let response = send_stop_request_inner(&mut stream, request, deadline);
    match response {
        Ok(Some(line)) => serde_json::from_str(&line)
            .map(Some)
            .map_err(|err| err.to_string()),
        Ok(None) => Ok(None),
        Err(err) if stop_request_error_allows_wait(&err) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

fn send_stop_request_inner(
    stream: &mut LocalStream,
    request: &serde_json::Value,
    deadline: Instant,
) -> std::io::Result<Option<String>> {
    stream.write_all(request.to_string().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let Some(read_timeout) = socket_timeout_until(deadline) else {
        return Ok(None);
    };
    if let Err(err) = stream.set_recv_timeout(Some(read_timeout)) {
        if stop_timeout_error_allows_wait(&err) {
            return Ok(None);
        }
        return Err(err);
    }

    let mut line = String::new();
    let bytes_read = BufReader::new(stream).read_line(&mut line)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

pub(crate) fn stop_timeout_error_allows_wait(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::InvalidInput
        || (cfg!(windows) && err.kind() == std::io::ErrorKind::Unsupported)
}

pub(crate) fn stop_request_error_allows_wait(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
    )
}

fn is_running_at(socket_path: &Path) -> bool {
    socket_path.exists() && crate::platform::ipc::connect_local_stream(socket_path).is_ok()
}

fn wait_until_stopped_until(socket_paths: &[PathBuf], deadline: Instant) -> bool {
    while Instant::now() < deadline {
        if socket_paths.iter().all(|path| !is_running_at(path)) {
            return true;
        }
        std::thread::sleep(STOP_WAIT_POLL.min(time_until(deadline)));
    }
    socket_paths.iter().all(|path| !is_running_at(path))
}

fn reachable_socket_paths(socket_paths: &[PathBuf]) -> Vec<PathBuf> {
    socket_paths
        .iter()
        .filter(|path| is_running_at(path))
        .cloned()
        .collect()
}

fn time_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn socket_timeout_until(deadline: Instant) -> Option<Duration> {
    socket_timeout_from_remaining(time_until(deadline))
}

pub(crate) fn socket_timeout_from_remaining(remaining: Duration) -> Option<Duration> {
    if remaining.is_zero() {
        return None;
    }
    Some(remaining.max(MIN_SOCKET_TIMEOUT))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{send_stop_request, LocalStream};
    use super::{
        socket_timeout_from_remaining, stop_request_error_allows_wait,
        stop_timeout_error_allows_wait, MIN_SOCKET_TIMEOUT, STOP_WAIT_TIMEOUT,
    };
    #[cfg(unix)]
    use std::io::{BufRead, BufReader};
    use std::time::Duration;
    #[cfg(unix)]
    use std::time::Instant;

    include!("tests/session_stop_test.rs");
}
