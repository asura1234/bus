//! Executed in the initialized pane shell, then replaced by the native TUI.
use super::{remote_argv, rpc::Rpc, runtime};
use crate::platform::owned_process::{
    acquire_writer_lock, ensure_owner_alive, inherit_writer_lock, supervised_command,
    terminate_group,
};
use std::{
    fs, io,
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let [directory, capture, argv @ ..] = args else {
        return Err(io::Error::other("Invalid native Codex launch"));
    };
    if argv.is_empty() || !Path::new(directory).is_absolute() || !Path::new(capture).is_absolute() {
        return Err(io::Error::other("Invalid native Codex launch identity"));
    }
    let control = UnixStream::connect(Path::new(directory).join("l"))?;
    let lock = writer_lock(Path::new(capture))?;
    ensure_owner_alive(&control)?;
    let socket = Path::new(directory).join("s");
    let endpoint = format!("unix://{}", socket.display());
    let mut server = Server::start(
        argv,
        &endpoint,
        Path::new(capture),
        &lock,
        control.try_clone()?,
    )?;
    let attached = server.ready(&socket).is_ok();
    let tui = if attached {
        remote_argv(argv, &endpoint)
    } else {
        // Startup never sent input. Retire the attempted backend before the
        // configured standalone TUI is allowed to become its writer.
        server.stop()?;
        ensure_owner_alive(&control)?;
        argv.to_vec()
    };
    fs::write(
        Path::new(directory).join("mode"),
        if attached {
            b"native".as_slice()
        } else {
            b"standalone".as_slice()
        },
    )?;
    let mut command = Command::new(&tui[0]);
    command.args(&tui[1..]);
    if !attached {
        inherit_writer_lock(&mut command, &lock);
    }
    // Both backend and TUI inherit this verified shell's cwd and environment.
    Err(command.exec())
}

fn writer_lock(capture: &Path) -> io::Result<fs::File> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match acquire_writer_lock(&capture.join("codex-native-writer.lock")) {
            Ok(lock) => return Ok(lock),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(writer_lock_error(capture, error)),
        }
    }
}

#[derive(Debug)]
struct WriterLockCause {
    message: String,
    source: io::Error,
}

impl std::fmt::Display for WriterLockCause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WriterLockCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn writer_lock_error(capture: &Path, error: io::Error) -> io::Error {
    let path = capture.join("codex-native-writer.lock");
    let expired = error.kind() == io::ErrorKind::WouldBlock;
    let kind = if expired {
        io::ErrorKind::TimedOut
    } else {
        error.kind()
    };
    let message = if expired {
        format!(
            "Codex writer lock still held after 5 seconds: {} ({error})",
            path.display()
        )
    } else {
        format!(
            "Cannot verify Codex writer ownership for {}: {error}",
            path.display()
        )
    };
    io::Error::new(
        kind,
        WriterLockCause {
            message,
            source: error,
        },
    )
}

struct Server {
    child: Child,
}

impl Server {
    fn start(
        argv: &[String],
        endpoint: &str,
        capture: &Path,
        lock: &fs::File,
        control: UnixStream,
    ) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(capture.join("codex-native.log"))?;
        let mut command = supervised_command(&runtime::server_argv(argv, endpoint), lock);
        let child = command
            .stdin(Stdio::from(OwnedFd::from(control)))
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()?;
        Ok(Self { child })
    }

    fn ready(&mut self, socket: &Path) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.child.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "Codex app-server exited before attachment",
                ));
            }
            if Rpc::connect(socket, deadline).is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Codex app-server did not accept attachment",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        terminate_group(&mut self.child, Duration::from_secs(3))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
#[path = "launcher_test.rs"]
mod tests;
