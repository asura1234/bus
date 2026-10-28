//! A socket pathname appears at bind, before listen. Readiness is a complete
//! status reply from the host we spawned, with a live scratch Bus behind it.
use super::protocol::{failure, Command};
use super::session::SessionPaths;
use crate::platform::ipc;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_STATUS_BYTES: usize = 64 * 1024;

pub(super) fn wait(
    paths: &SessionPaths,
    child: &mut Child,
    binary: &Path,
    token: &str,
    started: Instant,
    timeout: Duration,
) -> Value {
    let deadline = Instant::now() + timeout;
    match wait_status(paths, child, deadline) {
        Ok(mut status) => {
            status["ready_ms"] = json!(started.elapsed().as_millis() as u64);
            status
        }
        Err(detail) => {
            // Give a host reporting its own error time to finish teardown. A
            // hung host is killed and reaped before its scratch state is removed.
            let exit_by = deadline.min(Instant::now() + Duration::from_secs(20));
            while child.try_wait().ok().flatten().is_none() && Instant::now() < exit_by {
                std::thread::sleep(POLL);
            }
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
            // Also sweep after an early exit: the host may have died between
            // spawning its Bus and acquiring enough state to tear it down.
            super::stop_leftovers(paths, binary, token);
            let _ = paths.remove();
            failure(
                "start_failed",
                format!("session {:?} did not start: {detail}", paths.name),
            )
        }
    }
}

fn wait_status(
    paths: &SessionPaths,
    child: &mut Child,
    deadline: Instant,
) -> Result<Value, String> {
    let mut last_error = "host has not published its session metadata".to_owned();
    loop {
        if let Ok(error) = std::fs::read_to_string(paths.dir.join("host.error")) {
            return Err(error);
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let log = std::fs::read_to_string(paths.host_log()).unwrap_or_default();
            return Err(format!(
                "host exited with {status}: {last_error}; {}",
                log.lines().rev().take(20).collect::<Vec<_>>().join("\n")
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!("driver status deadline elapsed: {last_error}"));
        }
        if paths
            .read_info()
            .is_some_and(|info| info.host_pid == child.id())
        {
            let probe_by = deadline.min(Instant::now() + PROBE_TIMEOUT);
            match status(paths, probe_by) {
                Ok(status)
                    if status["ok"] == true
                        && status["alive"] == true
                        && status["host_pid"] == child.id() =>
                {
                    return Ok(status);
                }
                Ok(status) => {
                    return Err(format!("host did not report a live scratch Bus: {status}"))
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        // Poll a condition, never infer readiness from elapsed time.
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

struct Probe(ipc::LocalStream);

impl Drop for Probe {
    fn drop(&mut self) {
        ipc::discard_local_stream_output_on_close(&self.0);
    }
}

/// A peer that accepts without answering must not bypass the startup deadline.
fn status(paths: &SessionPaths, deadline: Instant) -> io::Result<Value> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let mut stream = Probe(ipc::connect_local_stream_timeout(
        &paths.socket(),
        remaining,
    )?);
    let mut command = serde_json::to_vec(&Command::Status).map_err(io::Error::other)?;
    command.push(b'\n');
    stream.0.write_all(&command)?;
    let mut reply = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "host status did not answer",
            ));
        }
        match ipc::poll_local_stream_read_count(&mut stream.0, &mut buffer)? {
            ipc::LocalStreamReadCount::Data(n) => {
                reply.extend_from_slice(&buffer[..n]);
                if reply.len() > MAX_STATUS_BYTES {
                    return Err(io::Error::other("host status exceeds 64 KiB"));
                }
                if reply.contains(&b'\n') {
                    return serde_json::from_slice(&reply).map_err(io::Error::other);
                }
            }
            ipc::LocalStreamReadCount::Closed => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "host sent no complete status",
                ));
            }
            ipc::LocalStreamReadCount::Pending => {
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
}

#[cfg(all(test, unix))]
#[path = "tests/startup_test.rs"]
mod tests;
