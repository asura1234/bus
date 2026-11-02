//! Prepare a shell-owned shared runtime; Bus owns only its lifetime channel.
use super::rpc::Rpc;
use portable_pty::CommandBuilder;
use std::{
    fs, io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Runtime {
    config: Option<Config>,
    session: Option<Session>,
}

struct Config {
    capture: PathBuf,
}

struct Session {
    directory: PathBuf,
    socket: PathBuf,
    rpc: Option<Rpc>,
    lifetime: Option<mpsc::Sender<()>>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl Runtime {
    pub(crate) fn shared_from_builder(
        cmd: &CommandBuilder,
    ) -> std::sync::Arc<std::sync::Mutex<Self>> {
        let environment = cmd
            .iter_extra_env_as_str()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<Vec<_>>();
        std::sync::Arc::new(std::sync::Mutex::new(
            Self::from_environment(&environment).unwrap_or_default(),
        ))
    }

    pub(crate) fn from_environment(env: &[(String, String)]) -> Option<Self> {
        if !env
            .iter()
            .any(|(key, value)| key == "BUS_CODEX_NATIVE" && value == "1")
        {
            return None;
        }
        let capture = env
            .iter()
            .find(|(key, _)| key == "BUS_CALLBACK_DIR")
            .map(|(_, value)| PathBuf::from(value))
            .filter(|path| path.is_absolute())?;
        Some(Self {
            config: Some(Config { capture }),
            session: None,
        })
    }

    pub(crate) fn launch(&mut self, argv: &[String]) -> io::Result<Vec<String>> {
        let Some(config) = self.config.as_ref() else {
            return Ok(argv.to_vec());
        };
        if argv
            .first()
            .is_none_or(|v| Path::new(v).file_name().is_none_or(|v| v != "codex"))
        {
            return Ok(argv.to_vec());
        }
        self.session.take();
        if !supports_native(&argv[1..]) {
            return Ok(argv.to_vec());
        }
        // This is inert provider preparation under the shell's guarded launch.
        // The helper runs only after that initialized shell accepts the command.
        let session = Session::prepare()?;
        let mut launch = vec![
            std::env::current_exe()?.to_string_lossy().into_owned(),
            "--bus-codex-runtime".into(),
            session.directory.to_string_lossy().into_owned(),
            config.capture.to_string_lossy().into_owned(),
        ];
        launch.extend_from_slice(argv);
        self.session = Some(session);
        Ok(launch)
    }

    pub(crate) fn prompt(
        &mut self,
        thread: &str,
        text: &str,
        steer: bool,
        message: &str,
        deadline: Instant,
        expected_turn: Option<&str>,
    ) -> Option<io::Result<()>> {
        let session = self.session.as_mut()?;
        if session.standalone() {
            return None;
        }
        let started = Instant::now();
        let result = session
            .connection(deadline)
            .and_then(|rpc| rpc.prompt(thread, text, steer, message, expected_turn))
            .map(|_| ());
        tracing::info!(
            event = "bus.codex.native_submit",
            api_request_id = message,
            thread_id = thread,
            steer,
            admitted = result.is_ok(),
            elapsed_us = started.elapsed().as_micros() as u64,
            "Codex app-server admission result"
        );
        session.rpc.take();
        Some(result)
    }

    pub(crate) fn has_session(&self) -> bool {
        self.session.is_some()
    }

    pub(crate) fn interrupt(&mut self, thread: &str, deadline: Instant) -> Option<io::Result<()>> {
        let session = self.session.as_mut()?;
        if session.standalone() {
            return None;
        }
        let result = session
            .connection(deadline)
            .and_then(|rpc| rpc.interrupt(thread));
        tracing::info!(
            event = "bus.codex.native_interrupt",
            thread_id = thread,
            admitted = result.is_ok(),
            "Codex app-server interrupt result"
        );
        session.rpc.take();
        Some(result)
    }
}

// Pure capability selection; does not create a provider process or input scope.
fn supports_native(args: &[String]) -> bool {
    let positions = crate::agents::providers::launch::argument_positions(
        crate::agents::providers::ProviderKind::Codex,
        args,
    );
    let resumed = positions
        .iter()
        .any(|&index| matches!(args[index].as_str(), "resume" | "fork"));
    !positions.iter().any(|&index| {
        let arg = args[index].as_str();
        matches!(arg, "-p" | "--profile" | "--remote")
            || arg.starts_with("--profile=")
            || (arg.starts_with("-p") && arg.len() > 2)
            || arg.starts_with("--remote=")
            || (resumed
                && (matches!(
                    arg,
                    "-s" | "--sandbox"
                        | "-a"
                        | "--ask-for-approval"
                        | "--yolo"
                        | "--dangerously-bypass-approvals-and-sandbox"
                        | "--full-auto"
                        | "--approve-for-me"
                ) || arg.starts_with("--sandbox=")
                    || arg.starts_with("--ask-for-approval=")
                    || (arg.starts_with("-s") && arg.len() > 2)
                    || (arg.starts_with("-a") && arg.len() > 2)))
    })
}

impl Session {
    fn prepare() -> io::Result<Self> {
        let token = crate::utils::time::digest(
            format!("{}:{}", std::process::id(), crate::utils::time::now_ns()).as_bytes(),
        );
        let directory = std::env::temp_dir().join(format!("bus-codex-{}", &token[..20]));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let listener = match UnixListener::bind(directory.join("l")) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(error);
            }
        };
        listener.set_nonblocking(true)?;
        let (stop, stopped) = mpsc::channel();
        let cleanup = directory.clone();
        let watcher = std::thread::spawn(move || {
            let mut control = None;
            while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                if control.is_none() {
                    match listener.accept() {
                        Ok((stream, _)) => control = Some(stream),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => break,
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // EOF reaches the helper's supervisor before the endpoint disappears.
            drop(control);
            drop(listener);
            let _ = fs::remove_dir_all(cleanup);
        });
        Ok(Self {
            socket: directory.join("s"),
            directory,
            rpc: None,
            lifetime: Some(stop),
            watcher: Some(watcher),
        })
    }

    fn standalone(&self) -> bool {
        fs::read(self.directory.join("mode")).is_ok_and(|mode| mode == b"standalone")
    }

    fn connection(&mut self, deadline: Instant) -> io::Result<&mut Rpc> {
        if self.rpc.is_none() {
            // An attached TUI has no independent backend. A disconnected pair
            // must not receive terminal fallback, even before any mutation.
            self.rpc = Some(Rpc::connect(&self.socket, deadline).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Codex attached runtime is unavailable; input was not sent",
                )
            })?);
        }
        let rpc = self.rpc.as_mut().expect("connected native RPC");
        rpc.set_deadline(deadline);
        Ok(rpc)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.lifetime.take();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        } else {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::super::rpc::tests::server_for;
    pub(crate) fn connected_runtime(socket: std::path::PathBuf) -> super::Runtime {
        super::Runtime {
            config: None,
            session: Some(super::Session {
                directory: socket.parent().unwrap().to_owned(),
                socket,
                rpc: None,
                lifetime: None,
                watcher: None,
            }),
        }
    }
}

#[cfg(test)]
#[path = "runtime_test.rs"]
mod tests;

pub(super) fn server_argv(argv: &[String], endpoint: &str) -> Vec<String> {
    let mut server = vec![
        argv[0].clone(),
        "app-server".into(),
        "--listen".into(),
        endpoint.into(),
    ];
    let args = &argv[1..];
    for index in crate::agents::providers::launch::argument_positions(
        crate::agents::providers::ProviderKind::Codex,
        args,
    ) {
        if matches!(
            args[index].as_str(),
            "-c" | "--config" | "--enable" | "--disable"
        ) {
            if let Some(value) = args.get(index + 1) {
                server.extend([args[index].clone(), value.clone()]);
            }
        } else if ["--config=", "--enable=", "--disable="]
            .iter()
            .any(|prefix| args[index].starts_with(prefix))
            || (args[index].starts_with("-c") && args[index].len() > 2)
        {
            server.push(args[index].clone());
        }
    }
    server
}

pub(super) fn acquire_launch_lock(path: &Path) -> io::Result<fs::File> {
    use std::os::fd::AsRawFd;
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Writer lock is not a regular file"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

pub(super) fn owned_command(argv: &[String], lock: &fs::File) -> Command {
    // The lifetime watcher keeps the original pipe, while the provider reads
    // /dev/null. Either provider exit or Bus EOF tears down the whole group.
    const SUPERVISOR: &str = r#"
cleanup() {
    trap '' HUP INT TERM
    kill -TERM 0 2>/dev/null
    /bin/sleep 0.3
    kill -KILL 0 2>/dev/null
}
trap cleanup EXIT HUP INT TERM
"$@" </dev/null &
provider=$!
( IFS= read -r lifetime || :; kill -TERM "$$" 2>/dev/null ) <&0 &
wait "$provider"
"#;
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", SUPERVISOR, "bus-owned-process"])
        .args(argv)
        .stdin(Stdio::piped())
        .process_group(0);
    inherit_writer_lock(&mut command, lock);
    command
}

pub(super) fn inherit_writer_lock(command: &mut Command, lock: &fs::File) {
    use std::os::fd::AsRawFd;
    let fd = lock.as_raw_fd();
    // F_DUPFD allocates a free descriptor with CLOEXEC cleared. It never
    // overwrites std::process's own exec-error pipe or any inherited handle.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_DUPFD, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// A lifetime socket carries no payload. Readability, including HUP, is EOF.
pub(super) fn ensure_owner_alive(control: &UnixStream) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: control.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if result > 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "Process owner stopped",
        ));
    }
    Ok(())
}

/// Retire the entire private group, including tools that outlive its leader.
pub(super) fn terminate_group(child: &mut Child, timeout: Duration) -> io::Result<()> {
    let group = -(child.id() as i32);
    unsafe {
        libc::kill(group, libc::SIGTERM);
    }
    let started = Instant::now();
    loop {
        child.try_wait()?;
        if unsafe { libc::kill(group, 0) } != 0 {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Process group has not retired",
            ));
        }
        if started.elapsed() >= Duration::from_millis(300) {
            unsafe {
                libc::kill(group, libc::SIGKILL);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
