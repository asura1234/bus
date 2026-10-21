//! Socket listener, owned lease and bounded client state machine.
#[cfg(test)]
use super::client::{request, request_with_timeout};
use super::protocol::{
    encode, poll_write, ControlledStream, Frame, FrameError, IO_TIMEOUT, LOCK_NAME,
    MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, POLL_INTERVAL, SOCKET_NAME, WAIT_SOCKET_NAME,
    WORKER_TIMEOUT,
};
pub(crate) use super::protocol::{ControlCall, Request, Response};
use crate::messaging::{coordinator::BusCommand, storage::io as storage_io};
use crate::platform::ipc;
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

// Independent finite pools: status followers cannot consume interactive slots.
const MAX_CLIENTS: usize = 16;
const CONTROL_COMMAND_BIT: u64 = 1 << 63;

pub(crate) struct Server {
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    _sockets: [OwnedSocket; 2],
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
    data_dir: &Path,
    commands: SyncSender<(u64, BusCommand)>,
) -> Result<Server, String> {
    storage_io::private_dir(data_dir).map_err(|e| e.to_string())?;
    ipc::validate_private_socket_directory(data_dir)
        .map_err(|e| format!("control_private_directory: {e}"))?;
    let lease = storage_io::lock(&data_dir.join(LOCK_NAME))
        .map_err(|e| format!("Cannot own Bus control endpoint: {e}"))?;
    let (interactive, socket) = bind_endpoint(data_dir, SOCKET_NAME)?;
    let (waiters, wait_socket) = bind_endpoint(data_dir, WAIT_SOCKET_NAME)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let thread = thread::Builder::new()
        .name("bus-control".into())
        .spawn(move || serve([interactive, waiters], commands, stop))
        .map_err(|e| e.to_string())?;
    Ok(Server {
        stopped,
        thread: Some(thread),
        _sockets: [socket, wait_socket],
        _lease: lease,
    })
}

fn bind_endpoint(data_dir: &Path, name: &str) -> Result<(ipc::LocalListener, OwnedSocket), String> {
    let path = data_dir.join(name);
    ipc::reclaim_stale_private_socket(&path, IO_TIMEOUT)
        .map_err(|e| format!("Cannot prepare Bus control endpoint: {e}"))?;
    let listener = ipc::bind_private_local_listener(&path)
        .map_err(|e| format!("Cannot bind Bus control endpoint {}: {e}", path.display()))?;
    let socket = OwnedSocket {
        identity: ipc::socket_file_identity(&path).map_err(|e| e.to_string())?,
        path,
    };
    ipc::restrict_socket_permissions(&socket.path, 0o600).map_err(|e| e.to_string())?;
    listener
        .set_nonblocking(ListenerNonblockingMode::Both)
        .map_err(|e| e.to_string())?;
    Ok((listener, socket))
}

struct Endpoint {
    listener: Option<ipc::LocalListener>,
    clients: Vec<Client>,
    poll_budget: Option<Arc<()>>,
}

fn serve(
    listeners: [ipc::LocalListener; 2],
    commands: SyncSender<(u64, BusCommand)>,
    stopped: Arc<AtomicBool>,
) {
    let mut endpoints = listeners
        .into_iter()
        .enumerate()
        .map(|(index, listener)| Endpoint {
            listener: Some(listener),
            clients: Vec::new(),
            poll_budget: (index == 1).then(|| Arc::new(())),
        })
        .collect::<Vec<_>>();
    let mut next_id = CONTROL_COMMAND_BIT;
    while !stopped.load(Ordering::Acquire) {
        // Interactive admission and replies run first, regardless of poll load.
        for endpoint in &mut endpoints {
            endpoint.poll(&commands, &mut next_id);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

impl Endpoint {
    fn poll(&mut self, commands: &SyncSender<(u64, BusCommand)>, next_id: &mut u64) {
        // Bound accept work and overload replies independently for each endpoint.
        for _ in 0..MAX_CLIENTS {
            let Some(listener) = &self.listener else {
                break;
            };
            // Excess callers wait in the listen backlog: closing an accepted
            // connection unanswered leaves its caller unable to tell whether
            // its request ran.
            if self.clients.len() > MAX_CLIENTS {
                break;
            }
            match listener.accept() {
                Ok(stream) if self.clients.len() == MAX_CLIENTS => {
                    // One extra bounded slot delivers overload errors without
                    // leaving unread Windows pipe writes in a linger pool.
                    let mut client = Client::new(stream);
                    if client.respond(Response::failure(
                        "",
                        "server_busy",
                        "Control connection limit reached",
                    )) {
                        self.clients.push(client);
                    }
                }
                Ok(stream) => self.clients.push(Client::new(stream)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    tracing::warn!(
                        event = "bus.control.transport.failed",
                        stage = "accept",
                        endpoint = if self.poll_budget.is_some() {
                            "status"
                        } else {
                            "interactive"
                        },
                        "Control listener failed; endpoint disabled"
                    );
                    // Retire only this listener; existing clients still get
                    // replies and the other endpoint remains available.
                    self.listener = None;
                    break;
                }
            }
        }
        self.clients
            .retain_mut(|client| client.poll(commands, next_id, self.poll_budget.as_ref()));
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
        /// A static label, never the caller's text.
        method: &'static str,
        started: Instant,
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

    fn poll(
        &mut self,
        commands: &SyncSender<(u64, BusCommand)>,
        next_id: &mut u64,
        poll_budget: Option<&Arc<()>>,
    ) -> bool {
        if Instant::now() >= self.deadline {
            return match &self.state {
                State::Reading(_) => self.respond(Response::failure(
                    "",
                    "request_timeout",
                    "Request deadline elapsed",
                )),
                State::Waiting {
                    id,
                    method,
                    started,
                    ..
                } => {
                    crate::messaging::perf::CONTROL_REQUEST.record_op(started.elapsed(), method);
                    self.respond(Response::failure(
                        id,
                        "coordinator_timeout",
                        "Coordinator deadline elapsed; the operation may still complete",
                    ))
                }
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
                self.dispatch_request(&bytes, commands, next_id, poll_budget)
            }

            State::Waiting {
                id,
                method,
                started,
                reply,
            } => match reply.try_recv() {
                Ok(response) => {
                    crate::messaging::perf::CONTROL_REQUEST.record_op(started.elapsed(), method);
                    self.respond(response)
                }
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

    fn dispatch_request(
        &mut self,
        bytes: &[u8],
        commands: &SyncSender<(u64, BusCommand)>,
        next_id: &mut u64,
        poll_budget: Option<&Arc<()>>,
    ) -> bool {
        let request: Request = match serde_json::from_slice(bytes) {
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
        if (request.method == "message.status") != poll_budget.is_some() {
            return self.respond(Response::failure(
                &request.id,
                "invalid_request",
                "Method belongs to the other control endpoint",
            ));
        }
        // A timed-out transport must not release a queued poll's capacity.
        // At most MAX_CLIENTS polls can occupy the shared coordinator queue.
        if poll_budget.is_some_and(|budget| Arc::strong_count(budget) > MAX_CLIENTS) {
            return self.respond(Response::failure(
                &request.id,
                "server_busy",
                "Status poll budget reached",
            ));
        }
        let id = request.id.clone();
        let method = crate::messaging::perf::control_method_label(&request.method);
        let (reply, receiver) = mpsc::sync_channel(1);
        let command_id = *next_id;
        *next_id = next_id.wrapping_add(1) | CONTROL_COMMAND_BIT;
        match commands.try_send((
            command_id,
            BusCommand::Control(ControlCall {
                request,
                reply,
                _poll_permit: poll_budget.cloned(),
            }),
        )) {
            Ok(()) => {
                self.state = State::Waiting {
                    id,
                    method,
                    started: Instant::now(),
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
}

#[cfg(test)]
#[path = "tests/server_test.rs"]
mod tests;
