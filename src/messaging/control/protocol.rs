//! Control envelopes, bounded JSON-line framing and shared IO budgets.
use crate::platform::ipc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::mpsc::SyncSender,
    time::Duration,
};

pub(super) const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub(super) const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) const WORKER_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(2);
/// The control socket in a session's data directory. Every session runs it;
/// the `--dev` flag only decides which methods answer.
pub(crate) const SOCKET_NAME: &str = "control.sock";
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
    pub(super) fn poll(
        &mut self,
        stream: &mut ipc::LocalStream,
        max: usize,
    ) -> Result<Option<Vec<u8>>, FrameError> {
        let mut buffer = [0; 64 * 1024];
        match ipc::poll_local_stream_read_count(stream, &mut buffer).map_err(FrameError::Io)? {
            ipc::LocalStreamReadCount::Data(n) => {
                let end = buffer[..n].iter().position(|byte| *byte == b'\n');
                let used = end.unwrap_or(n);
                if self.bytes.len() + used > max {
                    return Err(FrameError::TooLarge);
                }
                self.bytes.extend_from_slice(&buffer[..used]);
                Ok(end.map(|_| std::mem::take(&mut self.bytes)))
            }
            ipc::LocalStreamReadCount::Pending => Ok(None),
            ipc::LocalStreamReadCount::Closed => Err(FrameError::Io(io::Error::new(
                if self.bytes.is_empty() {
                    io::ErrorKind::UnexpectedEof
                } else {
                    io::ErrorKind::InvalidData
                },
                "Incomplete control frame",
            ))),
        }
    }
}

pub(super) fn poll_write(
    stream: &mut ipc::LocalStream,
    bytes: &[u8],
    offset: &mut usize,
) -> io::Result<bool> {
    // Bound each write so one large response does not monopolize a server pass.
    let end = bytes.len().min(*offset + 64 * 1024);
    match stream.write(&bytes[*offset..end]) {
        Ok(n) => *offset += n,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) => {}
        Err(e) => return Err(e),
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
