//! Prepare a shell-owned shared runtime; Bus owns only its lifetime channel.
use super::rpc::Rpc;
use std::{
    fs, io,
    os::unix::{fs::DirBuilderExt, net::UnixListener},
    path::{Path, PathBuf},
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

// Admission and owned-turn interruption are wired by the following train slices.
#[cfg_attr(not(test), allow(dead_code))]
struct Session {
    directory: PathBuf,
    socket: PathBuf,
    rpc: Option<Rpc>,
    lifetime: Option<mpsc::Sender<()>>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Runtime {
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
            crate::platform::fs::running_executable()?
                .to_string_lossy()
                .into_owned(),
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

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn interrupt(
        &mut self,
        thread: &str,
        deadline: Instant,
    ) -> Option<io::Result<Option<String>>> {
        let session = self.session.as_mut()?;
        if session.standalone() {
            return None;
        }
        let result = session
            .connection(deadline)
            .and_then(|rpc| rpc.interrupt(thread));
        match &result {
            Ok(_) => tracing::info!(
                event = "bus.codex.native_interrupt",
                thread_id = thread,
                confirmed = true,
                "Codex app-server interrupt completion result"
            ),
            Err(err) => tracing::info!(
                event = "bus.codex.native_interrupt",
                thread_id = thread,
                confirmed = false,
                error = %err,
                "Codex app-server interrupt completion result"
            ),
        }
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

#[cfg_attr(not(test), allow(dead_code))]
impl Session {
    fn prepare() -> io::Result<Self> {
        Self::prepare_at(crate::utils::time::now_ns())
    }

    fn prepare_at(timestamp: u128) -> io::Result<Self> {
        static NEXT_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let token = crate::utils::time::digest(
            format!("{}:{timestamp}:{sequence}", std::process::id()).as_bytes(),
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
#[allow(dead_code, unused_imports)]
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
