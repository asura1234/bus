//! The pane owns its server child; input and TUI share this one runtime.
use super::{remote_argv, rpc::Rpc};
use portable_pty::CommandBuilder;
use std::{
    collections::HashMap,
    fs, io,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Runtime {
    config: Option<Config>,
    session: Option<Session>,
}

struct Config {
    env: HashMap<String, String>,
    cwd: PathBuf,
}

struct Session {
    child: Child,
    directory: PathBuf,
    socket: PathBuf,
    rpc: Option<Rpc>,
    _writer_lock: fs::File,
}

impl Runtime {
    pub(crate) fn shared_from_builder(
        cmd: &CommandBuilder,
    ) -> std::sync::Arc<std::sync::Mutex<Self>> {
        let enabled = cmd
            .iter_extra_env_as_str()
            .any(|(k, v)| k == "BUS_CODEX_NATIVE" && v == "1");
        let config = enabled.then(|| Config {
            env: cmd
                .iter_full_env_as_str()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            cwd: cmd
                .get_cwd()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
        });
        std::sync::Arc::new(std::sync::Mutex::new(Self {
            config,
            session: None,
        }))
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
        let args = &argv[1..];
        if crate::agents::providers::launch::argument_positions(
            crate::agents::providers::ProviderKind::Codex,
            args,
        )
        .iter()
        .any(|&index| {
            matches!(args[index].as_str(), "-p" | "--profile" | "--remote")
                || args[index].starts_with("--profile=")
                || (args[index].starts_with("-p") && args[index].len() > 2)
                || args[index].starts_with("--remote=")
        }) {
            // App-server 0.164 explicitly rejects config profiles. Keep its
            // configured native TUI rather than silently changing runtime policy.
            return Ok(argv.to_vec());
        }
        // Only the guarded shell launcher calls this, after ownership admission.
        // Retire the previous pair before creating a replacement runtime.
        match Session::start(config, argv) {
            Ok(session) => {
                let endpoint = format!("unix://{}", session.socket.display());
                let argv = remote_argv(argv, &endpoint);
                self.session = Some(session);
                Ok(argv)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::PermissionDenied
                ) =>
            {
                Err(error)
            }
            Err(error) => {
                tracing::warn!(event="bus.codex.native_unavailable", kind=?error.kind(), "Codex app-server unavailable before TUI launch; retaining typed delivery");
                Ok(argv.to_vec())
            }
        }
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
        let rpc = session.connection(deadline)?;
        let started = Instant::now();
        let result = rpc
            .prompt(thread, text, steer, message, expected_turn)
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
        // The request may have been admitted. Never retry or type this input.
        session.rpc.take();
        Some(result)
    }

    pub(crate) fn has_session(&self) -> bool {
        self.session.is_some()
    }

    pub(crate) fn interrupt(&mut self, thread: &str, deadline: Instant) -> Option<io::Result<()>> {
        let session = self.session.as_mut()?;
        let result = session.connection(deadline)?.interrupt(thread);
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

impl Session {
    fn start(config: &Config, argv: &[String]) -> io::Result<Self> {
        let capture = config
            .env
            .get("BUS_CALLBACK_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| io::Error::other("Codex native launch has no callback identity"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let writer_lock = loop {
            match acquire_launch_lock(&capture.join("codex-native-writer.lock")) {
                Ok(lock) => break lock,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("Cannot verify Codex writer ownership: {}", error.kind()),
                    ))
                }
            }
        };
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(capture.join("codex-native.log"))?;
        let token = crate::utils::time::digest(
            format!("{}:{}", std::process::id(), crate::utils::time::now_ns()).as_bytes(),
        );
        let directory = std::env::temp_dir().join(format!("bus-codex-{}", &token[..20]));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let socket = directory.join("s");
        let endpoint = format!("unix://{}", socket.display());
        let mut command = owned_command(&server_argv(argv, &endpoint), &writer_lock);
        command
            .env_clear()
            .envs(&config.env)
            .current_dir(&config.cwd)
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .process_group(0);
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(error);
            }
        };
        let mut session = Self {
            child,
            directory,
            socket,
            rpc: None,
            _writer_lock: writer_lock,
        };
        loop {
            if session.child.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "Codex app-server exited before attachment",
                ));
            }
            if Rpc::connect(&session.socket, deadline).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Codex app-server did not accept attachment",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(session)
    }

    fn connection(&mut self, deadline: Instant) -> Option<&mut Rpc> {
        if self.rpc.is_none() {
            // A connect/initialize failure precedes a mutating RPC. Only this
            // boundary permits the caller's guarded typed fallback.
            self.rpc = Rpc::connect(&self.socket, deadline).ok();
        }
        let rpc = self.rpc.as_mut()?;
        rpc.set_deadline(deadline);
        Some(rpc)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let pid = self.child.id() as i32;
            // A private process group isolates provider tools from Bus itself.
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(300);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if matches!(self.child.try_wait(), Ok(None)) {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                let _ = self.child.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::super::rpc::tests::server_for;
    pub(crate) fn connected_runtime(socket: std::path::PathBuf) -> super::Runtime {
        let child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let writer_lock = super::acquire_launch_lock(&socket.with_extension("lock")).unwrap();
        super::Runtime {
            config: None,
            session: Some(super::Session {
                child,
                directory: socket.parent().unwrap().to_owned(),
                socket,
                rpc: None,
                _writer_lock: writer_lock,
            }),
        }
    }
}

#[cfg(test)]
#[path = "runtime_test.rs"]
mod tests;

fn acquire_launch_lock(path: &Path) -> io::Result<fs::File> {
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
        return Err(io::Error::other("Codex writer lock is not a regular file"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

fn owned_command(argv: &[String], lock: &fs::File) -> Command {
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
        .args(["-c", SUPERVISOR, "bus-codex-runtime"])
        .args(argv)
        .stdin(Stdio::piped());
    inherit_writer_lock(&mut command, lock);
    command
}

fn inherit_writer_lock(command: &mut Command, lock: &fs::File) {
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

fn server_argv(argv: &[String], endpoint: &str) -> Vec<String> {
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
