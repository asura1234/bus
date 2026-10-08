//! Opt-in socket listener, owned lease and bounded client state machine.
#[cfg(test)]
use super::client::{request, request_with_timeout};
use super::protocol::{
    encode, poll_write, ControlledStream, Frame, FrameError, IO_TIMEOUT, MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES, POLL_INTERVAL, WORKER_TIMEOUT,
};
pub(crate) use super::protocol::{DevCall, Request, Response};
use crate::ipc;
use crate::messaging::{coordinator::BusCommand, storage::io as storage_io};
use interprocess::local_socket::{traits::Listener as _, ListenerNonblockingMode};
#[cfg(test)]
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const MAX_CLIENTS: usize = 16;
const DEV_COMMAND_BIT: u64 = 1 << 63;

pub(crate) struct Server {
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    _socket: OwnedSocket,
    _lease: std::fs::File,
}

struct OwnedSocket {
    path: PathBuf,
    identity: ipc::SocketFileIdentity,
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        let _ = ipc::remove_socket_file_if_owned(&self.path, &self.identity);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Fields drop in declaration order: owned socket cleanup precedes lease release.
    }
}

pub(crate) fn start(
    enabled: bool,
    data_dir: &Path,
    commands: SyncSender<(u64, BusCommand)>,
) -> Result<Option<Server>, String> {
    if !enabled {
        return Ok(None);
    }
    storage_io::private_dir(data_dir).map_err(|e| e.to_string())?;
    ipc::validate_private_socket_directory(data_dir)
        .map_err(|e| format!("control_private_directory: {e}"))?;
    let lease = storage_io::lock(&data_dir.join("dev-control.lock"))
        .map_err(|e| format!("Cannot own Bus development endpoint: {e}"))?;
    let path = data_dir.join("dev-control.sock");
    ipc::reclaim_stale_private_socket(&path, IO_TIMEOUT)
        .map_err(|e| format!("Cannot prepare Bus development endpoint: {e}"))?;
    let listener = ipc::bind_private_local_listener(&path).map_err(|e| {
        format!(
            "Cannot bind Bus development endpoint {}: {e}",
            path.display()
        )
    })?;
    let socket = OwnedSocket {
        identity: ipc::socket_file_identity(&path).map_err(|e| e.to_string())?,
        path,
    };
    ipc::restrict_socket_permissions(&socket.path, 0o600).map_err(|e| e.to_string())?;
    listener
        .set_nonblocking(ListenerNonblockingMode::Both)
        .map_err(|e| e.to_string())?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let thread = thread::Builder::new()
        .name("bus-dev-control".into())
        .spawn(move || serve(listener, commands, stop))
        .map_err(|e| e.to_string())?;
    Ok(Some(Server {
        stopped,
        thread: Some(thread),
        _socket: socket,
        _lease: lease,
    }))
}

fn serve(
    listener: ipc::LocalListener,
    commands: SyncSender<(u64, BusCommand)>,
    stopped: Arc<AtomicBool>,
) {
    let mut clients = Vec::new();
    let mut next_id = DEV_COMMAND_BIT;
    while !stopped.load(Ordering::Acquire) {
        // Limit accept work per pass so connection floods cannot starve existing calls.
        for _ in 0..MAX_CLIENTS {
            match listener.accept() {
                Ok(stream) if clients.len() >= MAX_CLIENTS => {
                    // One extra bounded slot delivers overload errors without
                    // leaving unread Windows pipe writes in a linger pool.
                    if clients.len() == MAX_CLIENTS {
                        let mut client = Client::new(stream);
                        if client.respond(Response::failure(
                            "",
                            "server_busy",
                            "Development connection limit reached",
                        )) {
                            clients.push(client);
                        }
                    }
                }
                Ok(stream) => clients.push(Client::new(stream)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    tracing::warn!(
                        event = "bus.dev.transport.failed",
                        stage = "accept",
                        "Development control listener failed"
                    );
                    return;
                }
            }
        }
        clients.retain_mut(|client| client.poll(&commands, &mut next_id));
        thread::sleep(POLL_INTERVAL);
    }
}

struct Client {
    stream: ControlledStream,
    state: State,
    deadline: Instant,
}

enum State {
    Reading(Frame),
    Waiting {
        id: String,
        reply: mpsc::Receiver<Response>,
    },
    Writing {
        bytes: Vec<u8>,
        offset: usize,
    },
    Closing,
}

impl Client {
    fn new(stream: ipc::LocalStream) -> Self {
        Self {
            stream: ControlledStream(stream),
            state: State::Reading(Frame::default()),
            deadline: Instant::now() + IO_TIMEOUT,
        }
    }

    fn respond(&mut self, response: Response) -> bool {
        let bytes = encode(&response, MAX_RESPONSE_BYTES).or_else(|_| {
            encode(
                &Response::failure(
                    &response.id,
                    "response_too_large",
                    "Response exceeds 16 MiB",
                ),
                MAX_RESPONSE_BYTES,
            )
        });
        let Ok(bytes) = bytes else { return false };
        self.state = State::Writing { bytes, offset: 0 };
        self.deadline = Instant::now() + IO_TIMEOUT;
        true
    }

    fn poll(&mut self, commands: &SyncSender<(u64, BusCommand)>, next_id: &mut u64) -> bool {
        if Instant::now() >= self.deadline {
            return match &self.state {
                State::Reading(_) => self.respond(Response::failure(
                    "",
                    "request_timeout",
                    "Request deadline elapsed",
                )),
                State::Waiting { id, .. } => self.respond(Response::failure(
                    id,
                    "coordinator_timeout",
                    "Coordinator deadline elapsed; the operation may still complete",
                )),
                State::Writing { .. } | State::Closing => false,
            };
        }
        match &mut self.state {
            State::Reading(frame) => {
                let bytes = match frame.poll(&mut self.stream.0, MAX_REQUEST_BYTES) {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => return true,
                    Err(FrameError::TooLarge) => {
                        return self.respond(Response::failure(
                            "",
                            "request_too_large",
                            "Request exceeds 256 KiB",
                        ))
                    }
                    Err(FrameError::Io(e)) if e.kind() == io::ErrorKind::InvalidData => {
                        return self.respond(Response::failure(
                            "",
                            "invalid_request",
                            "Request must be one JSON line",
                        ))
                    }
                    Err(FrameError::Io(_)) => return false,
                };
                let request: Request = match serde_json::from_slice(&bytes) {
                    Ok(request) => request,
                    Err(_) => {
                        return self.respond(Response::failure(
                            "",
                            "invalid_request",
                            "Request must contain id, method and optional params",
                        ))
                    }
                };
                if request.id.is_empty() || request.method.is_empty() {
                    return self.respond(Response::failure(
                        &request.id,
                        "invalid_request",
                        "id and method must be nonempty",
                    ));
                }
                let id = request.id.clone();
                let (reply, receiver) = mpsc::sync_channel(1);
                let command_id = *next_id;
                *next_id = next_id.wrapping_add(1) | DEV_COMMAND_BIT;
                match commands.try_send((command_id, BusCommand::Dev(DevCall { request, reply }))) {
                    Ok(()) => {
                        self.state = State::Waiting {
                            id,
                            reply: receiver,
                        };
                        self.deadline = Instant::now() + WORKER_TIMEOUT;
                        true
                    }
                    Err(mpsc::TrySendError::Full(_)) => self.respond(Response::failure(
                        &id,
                        "coordinator_busy",
                        "Coordinator queue is full",
                    )),
                    Err(mpsc::TrySendError::Disconnected(_)) => self.respond(Response::failure(
                        &id,
                        "coordinator_unavailable",
                        "Coordinator has stopped",
                    )),
                }
            }
            State::Waiting { id, reply } => match reply.try_recv() {
                Ok(response) => self.respond(response),
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let response = Response::failure(
                        id,
                        "coordinator_unavailable",
                        "Coordinator did not return an outcome",
                    );
                    self.respond(response)
                }
            },
            State::Writing { bytes, offset } => match poll_write(&mut self.stream.0, bytes, offset)
            {
                Ok(true) => {
                    self.state = State::Closing;
                    true
                }
                Ok(false) => true,
                Err(_) => false,
            },
            // Keep replies counted until the peer closes after reading its
            // frame, or the original write deadline expires.
            State::Closing => matches!(
                ipc::poll_local_stream_read_count(&mut self.stream.0, &mut [0]),
                Ok(ipc::LocalStreamReadCount::Pending)
            ),
        }
    }
}

#[cfg(test)]
#[path = "tests/server_test.rs"]
mod tests;
