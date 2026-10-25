//! Control envelopes, bounded JSON-line framing and shared IO budgets.
use crate::platform::ipc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::{mpsc::SyncSender, Arc},
    time::{Duration, Instant},
};

pub(super) const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub(super) const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// Large frames may need several socket-buffer drains. Keep the idle budget
/// short while separately bounding a response that continues making progress.
pub(super) const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const WORKER_TIMEOUT: Duration = Duration::from_secs(15);

/// A launch can spend the native startup budget waiting for a real shell prompt.
/// Leave the existing worker allowance for setup, scheduling and the error reply.
pub(super) fn worker_timeout(method: &str) -> Duration {
    match method {
        "agent.add" | "agent.resume" => crate::protocol::api::AGENT_START_TIMEOUT + WORKER_TIMEOUT,
        _ => WORKER_TIMEOUT,
    }
}
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(2);
// Drain ready I/O in bounded bursts. Sleeping after each small I/O attempt makes a
// valid large response exceed IO_TIMEOUT when timers are coalesced by the OS.
const IO_BURST_CHUNKS: usize = 16;
/// The control socket in a session's data directory. Every session runs it;
/// the `--dev` flag only decides which methods answer.
pub(crate) const SOCKET_NAME: &str = "control.sock";
/// Status polling has independent admission from interactive control calls.
pub(super) const WAIT_SOCKET_NAME: &str = "wait.sock";
pub(super) const LOCK_NAME: &str = "control.lock";
/// The socket name of a Bus started by a build from before every session ran
/// the control socket. Such a Bus keeps it until it is restarted.
pub(super) const LEGACY_SOCKET_NAME: &str = "dev-control.sock";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Request {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Response {
    pub id: String,
    pub ok: bool,
    pub result: Value,
    pub error: Option<ControlError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ControlError {
    pub code: String,
    pub message: String,
}

impl Response {
    pub(crate) fn success(id: &str, result: Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result,
            error: None,
        }
    }

    pub(crate) fn failure(id: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: Value::Null,
            error: Some(ControlError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ControlCall {
    pub request: Request,
    pub reply: SyncSender<Response>,
    /// When the caller is answered `coordinator_timeout`; a read still queued
    /// then has no one to answer and is skipped. None never expires.
    pub(crate) deadline: Option<Instant>,
    /// Held through coordinator completion, even after the caller times out.
    pub(crate) _poll_permit: Option<Arc<()>>,
}

impl ControlCall {
    /// Transport dispatch time is anchored by the same finite reply deadline.
    /// Direct in-process callers without a deadline have no queue measurement.
    pub(crate) fn queued_for(&self) -> Option<Duration> {
        self.deadline
            .and_then(|deadline| deadline.checked_sub(worker_timeout(&self.request.method)))
            .map(|started| started.elapsed())
    }
}

pub(super) struct ControlledStream(pub(super) ipc::LocalStream);

impl Drop for ControlledStream {
    fn drop(&mut self) {
        ipc::discard_local_stream_output_on_close(&self.0);
    }
}

#[derive(Default)]
pub(super) struct Frame {
    bytes: Vec<u8>,
}

pub(super) enum FrameError {
    TooLarge,
    Io(io::Error),
}

impl Frame {
    pub(super) fn received_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub(super) fn poll(
        &mut self,
        stream: &mut ipc::LocalStream,
        max: usize,
    ) -> Result<Option<Vec<u8>>, FrameError> {
        let mut buffer = [0; 64 * 1024];
        for _ in 0..IO_BURST_CHUNKS {
            match ipc::poll_local_stream_read_count(stream, &mut buffer).map_err(FrameError::Io)? {
                ipc::LocalStreamReadCount::Data(n) => {
                    let end = buffer[..n].iter().position(|byte| *byte == b'\n');
                    let used = end.unwrap_or(n);
                    if self.bytes.len() + used > max {
                        return Err(FrameError::TooLarge);
                    }
                    self.bytes.extend_from_slice(&buffer[..used]);
                    if end.is_some() {
                        return Ok(Some(std::mem::take(&mut self.bytes)));
                    }
                }
                ipc::LocalStreamReadCount::Pending => return Ok(None),
                ipc::LocalStreamReadCount::Closed => {
                    return Err(FrameError::Io(io::Error::new(
                        if self.bytes.is_empty() {
                            io::ErrorKind::UnexpectedEof
                        } else {
                            io::ErrorKind::InvalidData
                        },
                        "Incomplete control frame",
                    )))
                }
            }
        }
        Ok(None)
    }
}

pub(super) fn poll_write(
    stream: &mut ipc::LocalStream,
    bytes: &[u8],
    offset: &mut usize,
) -> io::Result<bool> {
    // At most 1 MiB / 16 writes per pass preserves fairness between clients.
    for _ in 0..IO_BURST_CHUNKS {
        if *offset == bytes.len() {
            return Ok(true);
        }
        let end = bytes.len().min(*offset + 64 * 1024);
        match stream.write(&bytes[*offset..end]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Control write made no progress",
                ))
            }
            Ok(n) => *offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                break
            }
            Err(e) => return Err(e),
        }
    }
    Ok(*offset == bytes.len())
}

/// Stop serialization before allocating a payload beyond the frame budget.
pub(super) fn encode(value: &impl Serialize, max: usize) -> Result<Vec<u8>, serde_json::Error> {
    struct Limited {
        bytes: Vec<u8>,
        max: usize,
    }
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
                return Err(io::Error::other("control frame too large"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = Limited {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut output, value)?;
    output.bytes.push(b'\n');
    Ok(output.bytes)
}
