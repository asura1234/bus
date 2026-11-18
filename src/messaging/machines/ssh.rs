//! Direct SSH pipes to the ordinary control and pane sockets on a running Bus.
use super::config::validate_target;
use crate::messaging::control::{Request, Response};
use crate::platform::ipc::{self, LocalStream};
use interprocess::{
    local_socket::traits::{Listener as _, Stream as _},
    TryClone as _,
};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

const REQUEST_LIMIT: usize = 256 * 1024;
const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

fn ssh_command(target: &str, kind: &str) -> Result<Command, String> {
    validate_target(target)?;
    let mut command = crate::utils::env::command_on_path("ssh");
    command.args([
        "-T",
        "-oBatchMode=yes",
        "-oConnectTimeout=5",
        "-oConnectionAttempts=1",
        "-oServerAliveInterval=5",
        "-oServerAliveCountMax=1",
        "--",
        target,
        "bus",
        "--machine-bridge",
        kind,
    ]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // An SSH SendEnv wildcard must not bind the peer to the viewer's session.
    for (name, _) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if key.starts_with("BUS_") || key.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    Ok(command)
}

fn bounded_line(reader: &mut impl BufRead, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "SSH connection closed",
        ));
    }
    if bytes.len() > limit || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SSH frame is oversized or incomplete",
        ));
    }
    Ok(bytes)
}

/// One sequential RPC stream. Failure closes the stream; requests are never replayed.
pub(crate) struct ControlConnection {
    child: Child,
    writes: mpsc::SyncSender<Vec<u8>>,
    replies: mpsc::Receiver<Result<Response, String>>,
    closed: bool,
}

impl ControlConnection {
    pub(crate) fn connect(target: &str) -> Result<Self, String> {
        Self::spawn(ssh_command(target, "control")?)
    }

    fn spawn(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .spawn()
            .map_err(|error| format!("SSH unavailable: {error}"))?;
        let mut input = child.stdin.take().ok_or("SSH stdin unavailable")?;
        let output = child.stdout.take().ok_or("SSH stdout unavailable")?;
        let (writes, pending) = mpsc::sync_channel::<Vec<u8>>(1);
        let (answers, replies) = mpsc::channel();
        let write_errors = answers.clone();
        std::thread::spawn(move || {
            while let Ok(bytes) = pending.recv() {
                if let Err(error) = input.write_all(&bytes).and_then(|_| input.flush()) {
                    let _ = write_errors.send(Err(format!("SSH write failed: {error}")));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let reply = bounded_line(&mut output, RESPONSE_LIMIT)
                    .map_err(|error| format!("SSH read failed: {error}"))
                    .and_then(|bytes| {
                        serde_json::from_slice(&bytes)
                            .map_err(|error| format!("Invalid remote response: {error}"))
                    });
                let failed = reply.is_err();
                if answers.send(reply).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            writes,
            replies,
            closed: false,
        })
    }

    pub(crate) fn request(&mut self, request: &Request) -> Result<Response, String> {
        let timeout = if request.method == "agent.add"
            || request.method == "agent.resume"
            || request.method == "client.command"
        {
            crate::protocol::api::AGENT_START_TIMEOUT + Duration::from_secs(40)
        } else {
            Duration::from_secs(25)
        };
        self.request_with_timeout(request, timeout)
    }

    fn request_with_timeout(
        &mut self,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, String> {
        if self.closed {
            return Err("SSH connection is unavailable; nothing was queued".into());
        }
        let mut bytes = serde_json::to_vec(request).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > REQUEST_LIMIT {
            return Err("Remote request exceeds 256 KiB".into());
        }
        let result = self
            .writes
            .try_send(bytes)
            .map_err(|_| "SSH writer is unavailable".to_string())
            .and_then(|_| {
                self.replies.recv_timeout(timeout).map_err(|error| {
                    format!("SSH response failed: {error}; request was not retried")
                })
            })
            .and_then(|reply| reply)
            .and_then(|response| {
                if response.id != request.id || response.ok == response.error.is_some() {
                    Err("Remote response identity or outcome is invalid".into())
                } else {
                    Ok(response)
                }
            });
        if result.is_err() {
            self.close();
        }
        result
    }

    fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for ControlConnection {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) fn request(target: &str, request: &Request) -> Result<Response, String> {
    ControlConnection::connect(target)?.request(request)
}

pub(crate) struct TerminalConnection {
    child: Child,
    stopped: Arc<AtomicBool>,
    directory: PathBuf,
}

impl Drop for TerminalConnection {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(self.directory.join("pane.sock"));
        let _ = std::fs::remove_dir(&self.directory);
    }
}

pub(crate) fn connect_terminal(target: &str) -> Result<(LocalStream, TerminalConnection), String> {
    let directory = terminal_directory().map_err(|error| error.to_string())?;
    let result = connect_terminal_at(target, directory.clone());
    if result.is_err() {
        let _ = std::fs::remove_file(directory.join("pane.sock"));
        let _ = std::fs::remove_dir(directory);
    }
    result
}

fn terminal_directory() -> io::Result<PathBuf> {
    #[cfg(unix)]
    let temporary_root = PathBuf::from("/tmp");
    #[cfg(not(unix))]
    let temporary_root = std::env::temp_dir();
    // macOS /tmp is a system symlink. Resolve only the existing temp root,
    // before creating our own private child, so private_dir keeps rejecting
    // replaced/symlinked owned directories. The short root fits Unix sockets.
    let directory = temporary_root.canonicalize()?.join(format!(
        "bus-ssh-{}-{}",
        std::process::id(),
        crate::utils::time::now_ns()
    ));
    crate::messaging::storage::io::private_dir(&directory)?;
    Ok(directory)
}

fn connect_terminal_at(
    target: &str,
    directory: PathBuf,
) -> Result<(LocalStream, TerminalConnection), String> {
    let path = directory.join("pane.sock");
    let listener = ipc::bind_private_local_listener(&path).map_err(|error| error.to_string())?;
    let stream = ipc::connect_local_stream(&path).map_err(|error| error.to_string())?;
    let mut bridge = listener.accept().map_err(|error| error.to_string())?;
    bridge
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let mut read_bridge = bridge.try_clone().map_err(|error| error.to_string())?;
    let mut child = ssh_command(target, "terminal")?
        .spawn()
        .map_err(|error| format!("SSH unavailable: {error}"))?;
    let mut input = child.stdin.take().ok_or("SSH stdin unavailable")?;
    let mut output = child.stdout.take().ok_or("SSH stdout unavailable")?;
    let stopped = Arc::new(AtomicBool::new(false));
    let write_stopped = stopped.clone();
    std::thread::spawn(move || {
        let _ = copy_polling(&mut read_bridge, &mut input, &write_stopped);
        write_stopped.store(true, Ordering::Release);
    });
    let read_stopped = stopped.clone();
    std::thread::spawn(move || {
        let _ = copy_polling(&mut output, &mut bridge, &read_stopped);
        read_stopped.store(true, Ordering::Release);
    });
    Ok((
        stream,
        TerminalConnection {
            child,
            stopped,
            directory,
        },
    ))
}

fn copy_polling(
    reader: &mut impl Read,
    writer: &mut impl Write,
    stopped: &AtomicBool,
) -> io::Result<()> {
    let mut buffer = [0_u8; 16384];
    while !stopped.load(Ordering::Acquire) {
        let count = match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let mut offset = 0;
        while offset < count && !stopped.load(Ordering::Acquire) {
            match writer.write(&buffer[offset..count]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => offset += written,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        writer.flush()?;
    }
    Ok(())
}

/// Selects the normal session without creating, resuming or recording any session.
fn remote_root() -> Result<PathBuf, String> {
    if let Some(root) = crate::utils::env::bus_data_dir() {
        if !root.is_absolute() {
            return Err("Remote BUS_DATA_DIR is not absolute".into());
        }
        return Ok(root);
    }
    crate::messaging::storage::sessions::LocalSessionRegistry::new(
        crate::messaging::storage::sessions::default_base_dir()?,
    )
    .last()
    .map(|session| session.root)
}

pub(crate) fn bridge(kind: &str) -> io::Result<()> {
    let root = remote_root().map_err(io::Error::other)?;
    ipc::validate_private_socket_directory(&root)?;
    match kind {
        "control" => {
            let mut input = BufReader::new(io::stdin());
            let mut output = io::stdout().lock();
            loop {
                let bytes = match bounded_line(&mut input, REQUEST_LIMIT) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                    Err(error) => return Err(error),
                };
                let request: Request = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                let response =
                    crate::messaging::control::request(&root, &request).unwrap_or_else(|error| {
                        if crate::messaging::control::undelivered(&error) {
                            let mut response =
                                Response::failure(&request.id, "remote_unavailable", error);
                            response.result = serde_json::json!({"applied":false});
                            response
                        } else {
                            crate::messaging::machine_routes::unknown(
                                &request.id,
                                &format!("Destination acknowledgement was lost: {error}; request was not retried"),
                            )
                        }
                    });
                let mut bytes = serde_json::to_vec(&response).map_err(io::Error::other)?;
                bytes.push(b'\n');
                if bytes.len() > RESPONSE_LIMIT {
                    return Err(io::Error::other("Remote response exceeds 16 MiB"));
                }
                output.write_all(&bytes)?;
                output.flush()?;
            }
        }
        "terminal" => {
            // This is the exact ordinary Bus socket, not a new terminal daemon.
            let path = root.join("herdr-config/sessions/bus/herdr-client.sock");
            let mut stream = ipc::connect_local_stream_timeout(&path, Duration::from_secs(5))?;
            let mut input_stream = stream.try_clone()?;
            let stopped = Arc::new(AtomicBool::new(false));
            let input_stopped = stopped.clone();
            std::thread::spawn(move || {
                let _ = copy_polling(&mut io::stdin().lock(), &mut input_stream, &input_stopped);
                input_stopped.store(true, Ordering::Release);
            });
            copy_polling(&mut stream, &mut io::stdout().lock(), &stopped)
        }
        _ => Err(io::Error::other("Unknown machine bridge")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn ssh_terminal_directory_is_canonical_private_and_short_enough_for_a_socket() {
        use std::os::unix::fs::PermissionsExt;
        let directory = terminal_directory().unwrap();
        assert_eq!(directory, directory.canonicalize().unwrap());
        assert_eq!(
            directory.parent().unwrap(),
            PathBuf::from("/tmp").canonicalize().unwrap()
        );
        assert_eq!(
            std::fs::symlink_metadata(&directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        ipc::validate_private_socket_directory(&directory).unwrap();
        let socket = directory.join("pane.sock");
        std::os::unix::net::SocketAddr::from_pathname(&socket).unwrap();
        let listener = ipc::bind_private_local_listener(&socket).unwrap();
        let stream = ipc::connect_local_stream(&socket).unwrap();
        drop(stream);
        drop(listener);
        if socket.exists() {
            std::fs::remove_file(socket).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn ssh_argv_is_fixed_and_never_installs_or_launches_a_bus() {
        let command = ssh_command("dev@build", "terminal").unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(
            &args[args.len() - 5..],
            &["--", "dev@build", "bus", "--machine-bridge", "terminal"]
        );
        assert!(args.contains(&"-oBatchMode=yes"));
        assert!(args.contains(&"-oConnectionAttempts=1"));
    }

    #[test]
    fn framed_ssh_input_is_bounded_and_requires_a_full_line() {
        assert_eq!(bounded_line(&mut &b"{}\nrest"[..], 3).unwrap(), b"{}\n");
        assert!(bounded_line(&mut &b"{}"[..], 3).is_err());
        assert!(bounded_line(&mut &b"long\n"[..], 3).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_lost_ssh_response_is_never_replayed() {
        let effect = crate::utils::test_temp::unique_temp_path("machine-no-replay");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "read line; printf 'once\\n' >> \"$1\"; exit 0",
                "bus-test",
            ])
            .arg(&effect)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut connection = ControlConnection::spawn(command).unwrap();
        let request = Request {
            id: "once".into(),
            method: "message.send".into(),
            params: serde_json::json!({}),
        };
        assert!(connection
            .request_with_timeout(&request, Duration::from_secs(2))
            .is_err());
        assert!(connection.closed);
        assert!(connection
            .request_with_timeout(&request, Duration::from_secs(2))
            .unwrap_err()
            .contains("nothing was queued"));
        assert_eq!(std::fs::read(&effect).unwrap(), b"once\n");
        std::fs::remove_file(effect).unwrap();
    }
}
