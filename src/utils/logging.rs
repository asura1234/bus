use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::EnvFilter;

pub(crate) const DEFAULT_MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
pub(crate) const DEFAULT_RETAINED_LOG_FILES: usize = 0;

// Upstream input/toast diagnostics contain user content. Keep those payload dumps
// disabled even in dev mode; the rest of the runtime retains TRACE visibility.
pub(crate) const DEV_FILTER: &str =
    "bus=trace,bus::protocol::keys::host=off,bus::client::host_terminal::input=info,bus::private_payload=off";

/// Process policy supplied by startup composition, including inherited hook policy.
/// Logging itself does not read CLI, messaging or environment configuration.
pub(crate) struct LoggingOptions {
    pub(crate) filter: EnvFilter,
    pub(crate) max_bytes: u64,
    pub(crate) retained_files: usize,
    pub(crate) dev: bool,
}

#[cfg(test)]
pub(crate) mod test_capture {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub(crate) struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Capture {
        pub(crate) fn run(&self, action: impl FnOnce()) {
            self.run_filtered("trace", action);
        }
        pub(crate) fn run_filtered(&self, filter: &str, action: impl FnOnce()) {
            let writer = self.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, action);
        }
        pub(crate) fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }
}

pub(crate) fn init_file_logging_at(dir: PathBuf, file_name: &str, options: &LoggingOptions) {
    let Ok(make_writer) = RotatingFileMakeWriter::new(dir, file_name, options) else {
        return;
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(options.filter.clone())
        .with_writer(make_writer)
        .with_ansi(false)
        .with_target(true)
        .try_init();
}

pub(crate) use super::log_events::{
    api_request_completed, api_request_failed, api_request_started, api_wait_completed,
    api_wait_started, api_wait_timed_out, pane_exit_failed, pane_exited, pane_spawn_started,
    pane_spawned, session_clear_failed, session_cleared, session_restored, session_save_failed,
    session_saved, shutdown, tab_focused, tab_renamed, workspace_closed, workspace_created,
    workspace_focused, workspace_renamed,
};

pub(crate) fn startup(role: &'static str, dev: bool) {
    tracing::info!(
        target: "bus::utils::logging",
        event = "app.startup",
        subsystem = role,
        outcome = "started",
        pid = std::process::id(),
        dev,
        version = env!("CARGO_PKG_VERSION"),
        "herdr starting"
    );
}

struct RotatingFileMakeWriter {
    state: Arc<Mutex<RotatingFileState>>,
}

impl RotatingFileMakeWriter {
    fn new(dir: PathBuf, file_name: &str, options: &LoggingOptions) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let path = dir.join(file_name);
        let mut state = RotatingFileState {
            path,
            max_bytes: options.max_bytes,
            retained_files: options.retained_files,
            file: None,
            current_size: 0,
            disabled: false,
        };
        state.open_current_file()?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
        })
    }
}

impl<'a> MakeWriter<'a> for RotatingFileMakeWriter {
    type Writer = RotatingFileGuard;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingFileGuard {
            state: Arc::clone(&self.state),
        }
    }
}

struct RotatingFileGuard {
    state: Arc<Mutex<RotatingFileState>>,
}

impl Write for RotatingFileGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Ok(mut state) = self.state.lock() else {
            return Ok(buf.len());
        };
        if state.disabled {
            return Ok(buf.len());
        }
        if state.rotate_if_needed(buf.len() as u64).is_err() {
            state.disabled = true;
            return Ok(buf.len());
        }
        if let Some(file) = state.file.as_mut() {
            match file.write(buf) {
                Ok(written) => {
                    state.current_size = state.current_size.saturating_add(written as u64);
                    Ok(written)
                }
                Err(_) => {
                    state.disabled = true;
                    Ok(buf.len())
                }
            }
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let Ok(mut state) = self.state.lock() else {
            return Ok(());
        };
        if state.disabled {
            return Ok(());
        }
        match state.file.as_mut() {
            Some(file) => match file.flush() {
                Ok(()) => Ok(()),
                Err(_) => {
                    state.disabled = true;
                    Ok(())
                }
            },
            None => Ok(()),
        }
    }
}

struct RotatingFileState {
    path: PathBuf,
    max_bytes: u64,
    retained_files: usize,
    file: Option<File>,
    current_size: u64,
    disabled: bool,
}

impl RotatingFileState {
    fn rotate_if_needed(&mut self, incoming_len: u64) -> io::Result<()> {
        if self.file.is_none() {
            self.open_current_file()?;
        }
        if self.current_size.saturating_add(incoming_len) <= self.max_bytes {
            return Ok(());
        }
        self.rotate_files()?;
        self.open_current_file()
    }

    fn open_current_file(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.current_size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    fn rotate_files(&mut self) -> io::Result<()> {
        self.file.take();
        if self.retained_files == 0 {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            self.current_size = 0;
            return Ok(());
        }

        let oldest = rotated_log_path(&self.path, self.retained_files);
        match fs::remove_file(&oldest) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        for index in (1..=self.retained_files).rev() {
            let source = if index == 1 {
                self.path.clone()
            } else {
                rotated_log_path(&self.path, index - 1)
            };
            let target = rotated_log_path(&self.path, index);
            if !source.exists() {
                continue;
            }
            fs::rename(source, target)?;
        }

        self.current_size = 0;
        Ok(())
    }
}

fn rotated_log_path(path: &Path, index: usize) -> PathBuf {
    let suffix = format!(".{}", index);
    let file_name = path
        .file_name()
        .map(|name| {
            let mut name = name.to_os_string();
            name.push(&suffix);
            name
        })
        .unwrap_or_else(|| suffix.clone().into());
    path.with_file_name(file_name)
}

#[cfg(test)]
#[path = "tests/logging_test.rs"]
mod tests;
