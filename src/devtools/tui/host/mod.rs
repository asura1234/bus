//! The per-session host: owns the scratch Bus's hidden terminal, keeps its
//! output drained into the emulator, and answers one request at a time.
use super::input::{self, Button, Mods};
use super::protocol::{failure, Command, Target};
use super::screen::{self, Grid, Needle, Screen, CELL_HEIGHT_PX, CELL_WIDTH_PX};
use super::session::{scratch_env, SessionInfo, SessionPaths};
use super::trace::Trace;
use super::{bus_command, CommandOutput};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

mod actions;

/// How long an action waits for the first frame after its input.
const FRAME_WAIT: Duration = Duration::from_millis(500);
/// Quiet time that ends an action's settle, and the most it waits for it.
const SETTLE_QUIET: Duration = Duration::from_millis(50);
const SETTLE_MAX: Duration = Duration::from_millis(300);
const READY_TIMEOUT: Duration = Duration::from_secs(40);
/// Bus's room-UI double click needs both clicks within 400 ms.
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(60);

struct Output {
    screen: Screen,
    frames: u64,
    seq: u64,
    last_output: Instant,
    clipboard: Vec<String>,
    bells: u64,
    exited: Option<String>,
}

struct Host {
    paths: SessionPaths,
    info: SessionInfo,
    env: Vec<(String, OsString)>,
    output: Arc<(Mutex<Output>, Condvar)>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    trace: Arc<Mutex<Trace>>,
}

pub(super) struct HostOptions {
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub run_dir: PathBuf,
}

/// Entry of the hidden `bus --dev tui __host` process.
pub(super) fn run(root: &Path, options: HostOptions) -> i32 {
    let paths = SessionPaths::new(root, &options.name);
    match start(&paths, &options) {
        Ok(host) => host.serve(),
        Err(error) => {
            let _ = std::fs::write(paths.dir.join("host.error"), &error);
            eprintln!("bus tui host: {error}");
            1
        }
    }
}

fn start(paths: &SessionPaths, options: &HostOptions) -> Result<Host, String> {
    let binary = std::env::current_exe().map_err(|e| e.to_string())?;
    let env = scratch_env(paths, &super::session::inherited_env());
    super::fake_agents::install(&paths.bin_dir(), &binary).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&options.run_dir).map_err(|e| e.to_string())?;
    let trace = Trace::open(&options.run_dir, options.cols, options.rows)
        .map_err(|e| format!("cannot open run dir {}: {e}", options.run_dir.display()))?;

    let pair = native_pty_system()
        .openpty(pty_size(options.cols, options.rows))
        .map_err(|e| e.to_string())?;
    let mut command = CommandBuilder::new(&binary);
    command.arg("--dev");
    command.env_clear();
    for (key, value) in &env {
        command.env(key, value);
    }
    command.cwd(paths.work_dir());
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("cannot spawn bus: {e}"))?;
    drop(pair.slave);
    let bus_pid = child.process_id();
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
        pair.master.take_writer().map_err(|e| e.to_string())?,
    ));
    let output = Arc::new((
        Mutex::new(Output {
            screen: Screen::new(options.cols, options.rows).map_err(|e| e.to_string())?,
            frames: 0,
            seq: 0,
            last_output: Instant::now(),
            clipboard: Vec::new(),
            bells: 0,
            exited: None,
        }),
        Condvar::new(),
    ));
    let trace = Arc::new(Mutex::new(trace));
    let child = Arc::new(Mutex::new(child));
    spawn_pump(reader, output.clone(), writer.clone(), trace.clone());
    spawn_reaper(child.clone(), output.clone());

    let info = SessionInfo {
        name: options.name.clone(),
        host_pid: std::process::id(),
        bus_pid,
        data_dir: paths.data_dir(),
        run_dir: options.run_dir.clone(),
        binary,
        size: (options.cols, options.rows),
    };
    let host = Host {
        paths: paths.clone(),
        info,
        env,
        output,
        writer,
        master: pair.master,
        child,
        trace,
    };
    host.wait_ready()?;
    host.paths
        .write_info(&host.info)
        .map_err(|e| e.to_string())?;
    Ok(host)
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        // A real pixel size spares Bus its `CSI 16 t` cell-size query.
        pixel_width: cols.saturating_mul(CELL_WIDTH_PX as u16),
        pixel_height: rows.saturating_mul(CELL_HEIGHT_PX as u16),
    }
}

fn spawn_pump(
    mut reader: Box<dyn Read + Send>,
    output: Arc<(Mutex<Output>, Condvar)>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    trace: Arc<Mutex<Trace>>,
) {
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 65536];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            let bytes = &buffer[..count];
            if let Ok(mut trace) = trace.lock() {
                trace.output(bytes);
            }
            let (lock, ready) = &*output;
            let Ok(mut out) = lock.lock() else { break };
            let feed = out.screen.feed(bytes);
            out.frames += feed.frames;
            out.seq += 1;
            out.last_output = Instant::now();
            out.bells += u64::from(feed.bells);
            for write in feed.clipboard_writes {
                out.clipboard.push(decode_clipboard(&write));
            }
            drop(out);
            if !feed.responses.is_empty() {
                if let Ok(mut writer) = writer.lock() {
                    for response in feed.responses {
                        let _ = writer.write_all(&response);
                    }
                    let _ = writer.flush();
                }
            }
            ready.notify_all();
        }
        let (lock, ready) = &*output;
        if let Ok(mut out) = lock.lock() {
            out.exited.get_or_insert_with(|| "terminal closed".into());
        }
        ready.notify_all();
    });
}

fn spawn_reaper(
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    output: Arc<(Mutex<Output>, Condvar)>,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        let status = match child.lock() {
            Ok(mut child) => child.try_wait(),
            Err(_) => return,
        };
        if let Ok(Some(status)) = status {
            let (lock, ready) = &*output;
            if let Ok(mut out) = lock.lock() {
                out.exited = Some(format!("bus exited ({status:?})"));
            }
            ready.notify_all();
            return;
        }
    });
}

/// OSC 52 payloads arrive as `c;<base64>`; keep the decoded text.
fn decode_clipboard(write: &[u8]) -> String {
    use base64::Engine as _;
    let text = String::from_utf8_lossy(write);
    let payload = text.rsplit(';').next().unwrap_or(&text);
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_else(|_| text.into_owned())
}

impl Host {
    fn bus(&self, args: &[&str]) -> CommandOutput {
        bus_command(&self.info.binary, &self.env, args, Duration::from_secs(20))
    }

    fn wait_ready(&self) -> Result<(), String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(reason) = self.exited() {
                return Err(format!(
                    "{reason} before it was ready; screen:\n{}",
                    self.grid().text()
                ));
            }
            let state = self.bus(&["state"]);
            if state.ok() {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "Bus did not answer `bus state` within {}s ({}); screen:\n{}",
                    READY_TIMEOUT.as_secs(),
                    state.stderr.trim(),
                    self.grid().text()
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // The first room draws right after the control socket opens.
        let drawn = Instant::now() + Duration::from_secs(10);
        while self.grid().text().trim().is_empty() && Instant::now() < drawn {
            std::thread::sleep(Duration::from_millis(50));
        }
        self.quiet(Duration::from_millis(300), Duration::from_secs(5));
        Ok(())
    }

    fn exited(&self) -> Option<String> {
        self.output.0.lock().ok().and_then(|out| out.exited.clone())
    }

    fn grid(&self) -> Grid {
        self.output
            .0
            .lock()
            .map(|out| out.screen.grid())
            .unwrap_or_else(|_| Grid::blank(0, 0))
    }

    fn counters(&self) -> (u64, u64) {
        self.output
            .0
            .lock()
            .map(|out| (out.frames, out.seq))
            .unwrap_or((0, 0))
    }

    /// Waits until no output arrived for `quiet`, at most `max`.
    fn quiet(&self, quiet: Duration, max: Duration) {
        let deadline = Instant::now() + max;
        let (lock, ready) = &*self.output;
        let Ok(mut out) = lock.lock() else { return };
        loop {
            let idle = out.last_output.elapsed();
            let now = Instant::now();
            if idle >= quiet || now >= deadline {
                return;
            }
            let wait = (quiet - idle).min(deadline - now);
            out = match ready.wait_timeout(out, wait) {
                Ok((out, _)) => out,
                Err(_) => return,
            };
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), Value> {
        if let Some(reason) = self.exited() {
            return Err(failure("bus_exited", reason));
        }
        if let Ok(mut trace) = self.trace.lock() {
            trace.input(bytes);
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| failure("bus_exited", "terminal writer poisoned"))?;
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|e| failure("bus_exited", format!("terminal write failed: {e}")))
    }

    fn serve(mut self) -> i32 {
        let listener = match crate::platform::ipc::bind_local_listener(&self.paths.socket()) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = std::fs::write(
                    self.paths.dir.join("host.error"),
                    format!("cannot bind {}: {error}", self.paths.socket().display()),
                );
                self.teardown();
                return 1;
            }
        };
        use interprocess::local_socket::traits::Listener as _;
        loop {
            let Ok(stream) = listener.accept() else {
                continue;
            };
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let started = Instant::now();
            let (response, stop) = match serde_json::from_str::<Command>(&line) {
                Ok(Command::Stop) => (self.teardown(), true),
                Ok(command) => (self.handle(command.clone()), false),
                Err(error) => (failure("usage", format!("bad request: {error}")), false),
            };
            if let Ok(mut trace) = self.trace.lock() {
                trace.record(&line, &response, started.elapsed());
            }
            let mut stream = reader.into_inner();
            let _ = writeln!(stream, "{response}");
            let _ = stream.flush();
            if stop {
                drop(stream);
                let _ = self.paths.remove();
                return 0;
            }
        }
    }

    /// Quits the scratch Bus, stops its server, keeps its logs in the run dir.
    fn teardown(&mut self) -> Value {
        let mut report =
            json!({"ok": true, "session": self.info.name, "run_dir": self.info.run_dir});
        if self.exited().is_none() {
            report["quit"] = json!(self.bus(&["quit"]).ok());
            let deadline = Instant::now() + Duration::from_secs(8);
            while self.exited().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let stop = self.bus(&["stop"]);
        report["server_stopped"] = json!(stop.stdout.trim());
        if self.exited().is_none() {
            if let Ok(mut child) = self.child.lock() {
                let _ = child.kill();
            }
        }
        report["logs"] = json!(copy_logs(
            &self.info.data_dir,
            &self.info.run_dir.join("logs")
        ));
        let leftovers = super::session::pids_holding(&self.paths.dir);
        for pid in &leftovers {
            super::session::terminate(*pid);
        }
        if !leftovers.is_empty() {
            report["killed"] = json!(leftovers);
        }
        if let Ok(mut trace) = self.trace.lock() {
            trace.flush();
        }
        report
    }
}

/// Copies every `*.log` under the scratch data dir, keeping relative paths.
fn copy_logs(data_dir: &Path, into: &Path) -> usize {
    fn walk(dir: &Path, base: &Path, into: &Path, count: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, into, count);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".log") || n.contains(".log."))
            {
                if let Ok(relative) = path.strip_prefix(base) {
                    let target = into.join(relative);
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::copy(&path, &target).is_ok() {
                        *count += 1;
                    }
                }
            }
        }
    }
    let mut count = 0;
    walk(data_dir, data_dir, into, &mut count);
    count
}
