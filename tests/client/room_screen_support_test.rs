// Isolated dev-room client fixture; lifecycle cleanup addresses only its private root.
use super::room_host_screen;
use interprocess::local_socket::{prelude::*, Stream};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) fn unique_test_dir() -> PathBuf {
    #[cfg(unix)]
    let root = PathBuf::from("/tmp"); // Keep private socket paths below sockaddr_un limits.
    #[cfg(windows)]
    let root = std::env::temp_dir();
    root.join(format!(
        "bus-room-screen-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn control_stream(root: &Path) -> std::io::Result<Stream> {
    let path = root.join("dev-control.sock");
    #[cfg(unix)]
    let name = {
        use interprocess::local_socket::GenericFilePath;
        path.to_fs_name::<GenericFilePath>()?
    };
    #[cfg(windows)]
    let name = {
        use interprocess::local_socket::GenericNamespaced;
        path.to_string_lossy()
            .to_string()
            .to_ns_name::<GenericNamespaced>()?
    };
    Stream::connect(name)
}

fn scoped_command(base: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bus"));
    command.env("BUS_DATA_DIR", base.join("runtime"));
    command.env("XDG_CONFIG_HOME", base.join("config"));
    command.env("XDG_RUNTIME_DIR", base.join("runtime"));
    command.env("XDG_STATE_HOME", base.join("state"));
    for key in [
        "BUS_SESSION_ID",
        "HERDR_ENV",
        "HERDR_SESSION",
        "HERDR_SOCKET_PATH",
        "HERDR_CLIENT_SOCKET_PATH",
        "HERDR_CONFIG_PATH",
    ] {
        command.env_remove(key);
    }
    command
}

pub(super) struct RoomClient {
    child: Option<Box<dyn Child + Send + Sync>>,
    master: Option<Box<dyn MasterPty + Send>>,
    base: PathBuf,
    output: mpsc::Receiver<Vec<u8>>,
    pub(super) screen: room_host_screen::HostScreen,
}

impl RoomClient {
    pub(super) fn spawn(base: PathBuf) -> Self {
        let runtime = base.join("runtime");
        let config = base.join("config");
        // Let Bus create its private root; do not precreate a shared control directory.
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(config.join("herdr")).unwrap();
        fs::write(config.join("herdr/config.toml"), "onboarding = false\n").unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
        command.arg("--dev");
        command.env("BUS_DATA_DIR", &runtime);
        command.env("XDG_RUNTIME_DIR", &runtime);
        command.env("XDG_CONFIG_HOME", &config);
        command.env("XDG_STATE_HOME", base.join("state"));
        #[cfg(unix)]
        command.env("SHELL", "/bin/sh");
        command.cwd(&base);
        command.env("TERM", "xterm-256color");
        for key in [
            "BUS_SESSION_ID",
            "BUS_DEV_EXISTING_SERVER",
            "HERDR_ENV",
            "HERDR_SESSION",
            "HERDR_SOCKET_PATH",
            "HERDR_CLIENT_SOCKET_PATH",
            "HERDR_CONFIG_PATH",
        ] {
            command.env_remove(key);
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (sender, output) = mpsc::channel();
        // RoomClient owns the process/master on success and panic; closing its
        // PTY ends the reader without coupling this fixture to another suite.
        thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 || sender.send(bytes[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        let client = Self {
            child: Some(child),
            master: Some(pair.master),
            base,
            output,
            screen: room_host_screen::HostScreen::new(100, 30),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if control_stream(&runtime).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "isolated dev control did not start"
            );
            thread::sleep(Duration::from_millis(25));
        }
        client
    }

    pub(super) fn control(&self, method: &str, params: Value) {
        let mut command = scoped_command(&self.base);
        match method {
            "room.create" => {
                command.args(["room", "create", params["name"].as_str().unwrap()]);
            }
            "room.focus" => {
                command.args(["room", "focus", params["room"].as_str().unwrap()]);
            }
            "room.notes" => {
                command.args([
                    "room",
                    "notes",
                    params["room"].as_str().unwrap(),
                    "--text",
                    params["text"].as_str().unwrap(),
                ]);
            }
            _ => panic!("unsupported room-screen fixture command {method}"),
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("isolated control command timed out: {method}");
            }
            thread::sleep(Duration::from_millis(25));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{method}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(super) fn observe(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "client did not draw {needle:?}: {}",
                self.screen.text()
            );
            let bytes = self
                .output
                .recv_timeout(remaining)
                .expect("client PTY output");
            self.screen.write(&bytes);
            if self.screen.text().contains(needle) {
                // Drain the same repaint's trailing bytes before observing cells.
                while Instant::now() < deadline {
                    match self.output.recv_timeout(Duration::from_millis(50)) {
                        Ok(bytes) => self.screen.write(&bytes),
                        Err(_) => return,
                    }
                }
                panic!("client repaint never settled: {}", self.screen.text());
            }
        }
    }
}

impl Drop for RoomClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            drop(self.master.take());
            let _ = child.wait();
        }
        // Only this fixture's explicit private root is addressed. Never discover
        // or stop the user's instance through an inherited session environment.
        let stopped = scoped_command(&self.base)
            .arg("stop")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .is_some_and(|mut stop| {
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    match stop.try_wait() {
                        Ok(Some(status)) => return status.success(),
                        Err(_) => break,
                        Ok(None) if Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(25))
                        }
                        Ok(None) => break,
                    }
                }
                let _ = stop.kill();
                let _ = stop.wait();
                false
            });
        if stopped {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
}
