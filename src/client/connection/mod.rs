pub(super) mod agent_seen;
pub(crate) mod bootstrap;
pub(super) mod handshake;
pub(super) mod requests;
mod state;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::TryClone as _;
use tracing::warn;

use crate::client::events::ClientLoopEvent;
use crate::client::ClientError;
use crate::platform::ipc::LocalStream;
use crate::protocol::wire::{self, ClientMessage};

pub(in crate::client) fn start_endpoint_transport(
    stream: LocalStream,
    lifetime: impl Send + 'static,
    event_tx: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
    max_frame_size: usize,
) -> Result<bootstrap::NativeEndpointTransport, ClientError> {
    let reader = stream.try_clone().map_err(ClientError::ConnectionFailed)?;
    let transport = bootstrap::NativeEndpointTransport::with_lifetime(stream, lifetime)
        .map_err(ClientError::ConnectionFailed)?;
    let stopped = transport.stop_handle();
    let event_tx = event_tx.clone();
    std::thread::Builder::new()
        .name("endpoint-reader".into())
        .spawn(move || {
            server_reader_thread(reader, event_tx, &stopped, max_frame_size);
        })
        .map_err(ClientError::ConnectionFailed)?;
    Ok(transport)
}

/// Reads complete frames while retaining partial-read progress across nonblocking polls.
pub(in crate::client) fn server_reader_thread(
    mut stream: LocalStream,
    event_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
    should_quit: &Arc<AtomicBool>,
    max_frame_size: usize,
) {
    if stream.set_nonblocking(true).is_err() {
        let _ = event_tx.blocking_send(ClientLoopEvent::ServerDisconnected);
        return;
    }

    let mut stream = EndpointReader {
        stream: &mut stream,
        stopped: should_quit,
    };
    loop {
        if should_quit.load(Ordering::Acquire) {
            break;
        }

        match wire::read_message(&mut stream, max_frame_size) {
            Ok(msg) => {
                if event_tx
                    .blocking_send(ClientLoopEvent::ServerMessage {
                        message: Box::new(msg),
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(wire::FramingError::UnexpectedEof) => {
                let _ = event_tx.blocking_send(ClientLoopEvent::ServerDisconnected);
                break;
            }
            Err(wire::FramingError::Io(err)) if err.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(err) => {
                warn!(err = %err, "server read error");
                let _ = event_tx.blocking_send(ClientLoopEvent::ServerDisconnected);
                break;
            }
        }
    }
}

struct EndpointReader<'a> {
    stream: &'a mut LocalStream,
    stopped: &'a AtomicBool,
}

impl io::Read for EndpointReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return Ok(0);
            }
            match crate::platform::ipc::poll_local_stream_read_count(self.stream, buffer)? {
                crate::platform::ipc::LocalStreamReadCount::Data(count) => return Ok(count),
                crate::platform::ipc::LocalStreamReadCount::Closed => return Ok(0),
                crate::platform::ipc::LocalStreamReadCount::Pending => {
                    crate::platform::wait_client_stream_readable(self.stream)?;
                }
            }
        }
    }
}

pub(in crate::client) fn write_to_local_server(
    stream: &mut LocalStream,
    msg: &ClientMessage,
) -> io::Result<()> {
    wire::write_message(stream, msg).map_err(|error| io::Error::other(error.to_string()))
}

pub(in crate::client) trait ClientMessageSink {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()>;
}

impl ClientMessageSink for LocalStream {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()> {
        write_to_local_server(self, message)
    }
}

impl ClientMessageSink for bootstrap::ServerConnection {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()> {
        self.send(message)
    }
}

pub(in crate::client) fn write_to_server(
    stream: &mut impl ClientMessageSink,
    msg: &ClientMessage,
) -> io::Result<()> {
    stream.send_client_message(msg)
}
