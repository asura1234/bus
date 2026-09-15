//! Private, opt-in development control for the Bus coordinator.
use super::runtime::BusCommand;
use crate::ipc;
use interprocess::local_socket::{traits::Listener as _, ListenerNonblockingMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIENTS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const WORKER_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(2);
const DEV_COMMAND_BIT: u64 = 1 << 63;

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
pub(crate) struct DevCall {
    pub request: Request,
    pub reply: SyncSender<Response>,
}

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

struct ControlledStream(ipc::LocalStream);

impl Drop for ControlledStream {
    fn drop(&mut self) {
        ipc::discard_local_stream_output_on_close(&self.0);
    }
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
    super::io::private_dir(data_dir).map_err(|e| e.to_string())?;
    ipc::validate_private_socket_directory(data_dir)
        .map_err(|e| format!("control_private_directory: {e}"))?;
    let lease = super::io::lock(&data_dir.join("dev-control.lock"))
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
        format!("control_unavailable: connect to a private existing bus --dev data directory: {e}")
    })?;
    let bytes = encode(request, MAX_REQUEST_BYTES)
        .map_err(|_| "request_too_large: request exceeds 256 KiB".to_string())?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("request_timeout: request deadline elapsed".into());
    }
    let stream = ipc::connect_local_stream_timeout(
        &data_dir.join("dev-control.sock"),
        remaining.min(IO_TIMEOUT),
    )
    .map_err(|e| format!("control_unavailable: connect to an existing bus --dev instance: {e}"))?;
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

#[derive(Default)]
struct Frame {
    bytes: Vec<u8>,
}

enum FrameError {
    TooLarge,
    Io(io::Error),
}

impl Frame {
    fn poll(
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

fn poll_write(stream: &mut ipc::LocalStream, bytes: &[u8], offset: &mut usize) -> io::Result<bool> {
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
fn encode(value: &impl Serialize, max: usize) -> Result<Vec<u8>, serde_json::Error> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc;
    use interprocess::local_socket::traits::Stream as _;
    use serde_json::json;
    use std::{
        io::Write,
        path::PathBuf,
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            // Hex keeps the socket path short; pid and counter separate parallel processes.
            let dir = std::env::temp_dir().join(format!(
                "bdc-{:x}-{:x}-{:x}",
                std::process::id(),
                super::super::io::now_ns(),
                NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            Self(dir)
        }
        fn socket(&self) -> PathBuf {
            self.0.join("dev-control.sock")
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn call(id: &str) -> Request {
        Request {
            id: id.into(),
            method: "state.get".into(),
            params: Value::Null,
        }
    }

    fn read_response(mut stream: ipc::LocalStream) -> Response {
        read_response_open(&mut stream)
    }

    fn read_response_open(stream: &mut ipc::LocalStream) -> Response {
        stream.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bytes = Vec::new();
        let mut buf = [0; 8192];
        loop {
            match ipc::poll_local_stream_read_count(stream, &mut buf).unwrap() {
                ipc::LocalStreamReadCount::Data(n) => {
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                        return serde_json::from_slice(&bytes[..end]).unwrap();
                    }
                }
                ipc::LocalStreamReadCount::Pending => thread::sleep(Duration::from_millis(2)),
                ipc::LocalStreamReadCount::Closed => panic!("closed without response"),
            }
            assert!(Instant::now() < deadline, "response deadline elapsed");
        }
    }

    #[test]
    fn disabled_control_does_not_create_directory_or_listener() {
        let dir = TestDir::new();
        let (tx, _rx) = mpsc::sync_channel(1);
        assert!(start(false, &dir.0, tx).unwrap().is_none());
        assert!(!dir.0.exists());
    }

    #[test]
    fn client_cannot_enable_target_or_remove_existing_endpoint() {
        let dir = TestDir::new();
        assert!(request(&dir.0, &call("missing")).is_err());
        assert!(!dir.0.exists());
        std::fs::create_dir(&dir.0).unwrap();
        std::fs::write(dir.socket(), b"existing endpoint marker").unwrap();
        assert!(request(&dir.0, &call("stopped")).is_err());
        assert_eq!(
            std::fs::read(dir.socket()).unwrap(),
            b"existing endpoint marker"
        );
    }

    #[test]
    fn roundtrip_returns_exact_worker_outcome_with_disjoint_command_ids() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(2);
        let server = start(true, &dir.0, tx).unwrap().unwrap();
        let worker = thread::spawn(move || {
            for expected_id in ["snapshot", "bad-command"] {
                let (command_id, BusCommand::Dev(dev)) =
                    rx.recv_timeout(Duration::from_secs(5)).unwrap()
                else {
                    panic!("wrong coordinator command");
                };
                assert!(command_id & (1_u64 << 63) != 0);
                assert_eq!(dev.request.id, expected_id);
                assert_eq!(dev.request.method, "state.get");
                let response = if expected_id == "snapshot" {
                    Response::success(expected_id, json!({"revision": 7, "room": "alpha"}))
                } else {
                    Response::failure(expected_id, "invalid_params", "Room is missing")
                };
                dev.reply.try_send(response).unwrap();
            }
        });
        let ok = request(&dir.0, &call("snapshot")).unwrap();
        assert_eq!(ok.id, "snapshot");
        assert!(ok.ok);
        assert_eq!(ok.result, json!({"revision": 7, "room": "alpha"}));
        assert!(ok.error.is_none());
        let failed = request(&dir.0, &call("bad-command")).unwrap();
        assert!(!failed.ok);
        assert_eq!(failed.id, "bad-command");
        assert_eq!(failed.error.unwrap().code, "invalid_params");
        worker.join().unwrap();
        drop(server);
        assert!(!dir.socket().exists());
    }

    #[test]
    fn malformed_and_idle_clients_do_not_delay_healthy_requests() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(2);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let _idle = ipc::connect_local_stream(&dir.socket()).unwrap();
        let mut malformed = ipc::connect_local_stream(&dir.socket()).unwrap();
        malformed.write_all(b"{secret invalid json}\n").unwrap();
        let failed = read_response(malformed);
        assert!(!failed.ok);
        let error = failed.error.unwrap();
        assert_eq!(error.code, "invalid_request");
        assert!(!error.message.contains("secret"));
        let worker = thread::spawn(move || {
            let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
                panic!()
            };
            dev.reply
                .try_send(Response::success(&dev.request.id, json!("healthy")))
                .unwrap();
        });
        assert_eq!(
            request(&dir.0, &call("healthy")).unwrap().result,
            json!("healthy")
        );
        worker.join().unwrap();
    }

    #[test]
    fn oversized_request_is_rejected_without_coordinator_dispatch() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let mut stream = ipc::connect_local_stream(&dir.socket()).unwrap();
        stream.write_all(&vec![b'x'; 256 * 1024 + 1]).unwrap();
        let response = read_response(stream);
        assert_eq!(response.error.unwrap().code, "request_too_large");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let mut large = call("large");
        large.params = json!("x".repeat(256 * 1024));
        assert!(request(&dir.0, &large)
            .unwrap_err()
            .contains("request_too_large"));
    }

    #[test]
    fn full_or_disconnected_coordinator_returns_explicit_error() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(0);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        assert_eq!(
            request(&dir.0, &call("full")).unwrap().error.unwrap().code,
            "coordinator_busy"
        );
        drop(rx);
        assert_eq!(
            request(&dir.0, &call("gone")).unwrap().error.unwrap().code,
            "coordinator_unavailable"
        );
    }

    #[test]
    fn oversized_worker_response_is_replaced_with_bounded_error() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let worker = thread::spawn(move || {
            let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
                panic!()
            };
            dev.reply
                .try_send(Response::success(
                    &dev.request.id,
                    json!("x".repeat(16 * 1024 * 1024)),
                ))
                .unwrap();
        });
        let failed = request(&dir.0, &call("large")).unwrap();
        assert_eq!(failed.id, "large");
        assert_eq!(failed.error.unwrap().code, "response_too_large");
        worker.join().unwrap();
    }

    #[test]
    fn second_server_cannot_replace_a_live_listener() {
        let dir = TestDir::new();
        let (tx, _rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx.clone()).unwrap().unwrap();
        let before = ipc::socket_file_identity(&dir.socket()).unwrap();
        assert!(start(true, &dir.0, tx).is_err());
        assert_eq!(ipc::socket_file_identity(&dir.socket()).unwrap(), before);
    }

    #[test]
    fn dropping_server_cancels_idle_clients_and_preserves_replacement_endpoint() {
        let dir = TestDir::new();
        let (tx, _rx) = mpsc::sync_channel(1);
        let server = start(true, &dir.0, tx).unwrap().unwrap();
        let _idle = ipc::connect_local_stream(&dir.socket()).unwrap();
        std::fs::rename(dir.socket(), dir.0.join("old.sock")).unwrap();
        std::fs::write(dir.socket(), b"replacement endpoint").unwrap();
        let began = Instant::now();
        drop(server);
        assert!(began.elapsed() < Duration::from_secs(2));
        assert_eq!(
            std::fs::read(dir.socket()).unwrap(),
            b"replacement endpoint"
        );
    }

    #[test]
    fn idle_request_expires_without_dispatch() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let idle = ipc::connect_local_stream(&dir.socket()).unwrap();
        assert_eq!(read_response(idle).error.unwrap().code, "request_timeout");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn unanswered_coordinator_request_has_finite_deadline() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let result = request(&dir.0, &call("never-finished")).unwrap();
        assert_eq!(result.error.unwrap().code, "coordinator_timeout");
        let (_, BusCommand::Dev(dev)) = rx.try_recv().unwrap() else {
            panic!()
        };
        assert!(dev
            .reply
            .try_send(Response::success("never-finished", Value::Null))
            .is_err());
    }

    #[test]
    fn active_connection_limit_rejects_excess_clients() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(16);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let mut clients = Vec::new();
        let mut pending = Vec::new();
        for id in 0..16 {
            let mut client = ipc::connect_local_stream(&dir.socket()).unwrap();
            writeln!(
                client,
                "{}",
                serde_json::to_string(&call(&format!("pending-{id}"))).unwrap()
            )
            .unwrap();
            pending.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
            clients.push(client);
        }
        let excess = ipc::connect_local_stream(&dir.socket()).unwrap();
        assert_eq!(read_response(excess).error.unwrap().code, "server_busy");
        assert_eq!(pending.len(), 16);
    }

    #[test]
    fn caller_deadline_bounds_an_unanswered_request() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let path = dir.0.clone();
        let started = Instant::now();
        let client = thread::spawn(move || {
            request_with_timeout(&path, &call("deadline"), Duration::from_millis(100))
        });
        let pending = rx.recv_timeout(Duration::from_secs(1));
        let error = client.join().unwrap().unwrap_err();
        assert!(error.contains("response_timeout"), "{error}");
        assert!(
            pending.is_ok(),
            "request must reach the real coordinator boundary"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn client_rejects_a_response_for_another_request() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(1);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let worker = thread::spawn(move || {
            let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
                panic!()
            };
            dev.reply
                .try_send(Response::success(
                    "some-other-request",
                    json!({"accepted":true}),
                ))
                .unwrap();
        });
        assert!(request(&dir.0, &call("expected"))
            .unwrap_err()
            .contains("invalid_response"));
        worker.join().unwrap();
    }

    #[test]
    fn completed_responses_remain_counted_until_peers_close() {
        let dir = TestDir::new();
        let (tx, rx) = mpsc::sync_channel(16);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        let mut clients = Vec::new();
        for id in 0..16 {
            let mut stream = ipc::connect_local_stream(&dir.socket()).unwrap();
            writeln!(
                stream,
                "{}",
                serde_json::to_string(&call(&format!("retain-{id}"))).unwrap()
            )
            .unwrap();
            let (_, BusCommand::Dev(dev)) = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
                panic!()
            };
            dev.reply
                .try_send(Response::success(&dev.request.id, json!("ready")))
                .unwrap();
            assert!(read_response_open(&mut stream).ok);
            clients.push(stream);
        }
        let excess = ipc::connect_local_stream(&dir.socket()).unwrap();
        assert_eq!(read_response(excess).error.unwrap().code, "server_busy");
    }

    #[test]
    fn startup_reclaims_a_closed_listener_after_obtaining_its_lease() {
        let dir = TestDir::new();
        super::super::io::private_dir(&dir.0).unwrap();
        let old = ipc::bind_private_local_listener(&dir.socket()).unwrap();
        drop(old);
        assert!(
            dir.socket().exists(),
            "fixture must retain the crashed listener endpoint"
        );
        let (tx, _rx) = mpsc::sync_channel(0);
        let _server = start(true, &dir.0, tx).unwrap().unwrap();
        assert_eq!(
            request(&dir.0, &call("recovered"))
                .unwrap()
                .error
                .unwrap()
                .code,
            "coordinator_busy"
        );
    }

    #[test]
    fn startup_preserves_a_live_listener_without_our_lease() {
        let dir = TestDir::new();
        super::super::io::private_dir(&dir.0).unwrap();
        let _listener = ipc::bind_private_local_listener(&dir.socket()).unwrap();
        let identity = ipc::socket_file_identity(&dir.socket()).unwrap();
        let (tx, _rx) = mpsc::sync_channel(0);
        assert!(start(true, &dir.0, tx).is_err());
        assert_eq!(ipc::socket_file_identity(&dir.socket()).unwrap(), identity);
    }

    #[test]
    fn startup_preserves_a_non_socket_endpoint_file() {
        let dir = TestDir::new();
        super::super::io::private_dir(&dir.0).unwrap();
        std::fs::write(dir.socket(), b"user data").unwrap();
        let (tx, _rx) = mpsc::sync_channel(0);
        assert!(start(true, &dir.0, tx).is_err());
        assert_eq!(std::fs::read(dir.socket()).unwrap(), b"user data");
    }
}
