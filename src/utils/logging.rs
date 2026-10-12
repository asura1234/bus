use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime};

use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::EnvFilter;

pub(crate) const DEFAULT_MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
pub(crate) const DEFAULT_RETAINED_LOG_FILES: usize = 0;
/// Dev builds keep compressed archives: the live client measured ~1 MB/min of
/// logs, so a few numbered 5 MB files held only ~20 minutes.
pub(crate) const DEV_MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const DEV_LOG_ARCHIVE: ArchivePolicy = ArchivePolicy {
    max_age: Duration::from_secs(72 * 60 * 60),
    max_total_bytes: 512 * 1024 * 1024,
};

// Upstream input/toast diagnostics contain user content. Keep those payload dumps
// disabled even in dev mode; the rest of the runtime retains TRACE visibility.
pub(crate) const DEV_FILTER: &str =
    "bus=trace,bus::protocol::keys::host=off,bus::client::host_terminal::input=info,bus::private_payload=off";

/// Process policy supplied by startup composition, including inherited hook policy.
/// Logging itself does not read CLI, messaging or environment configuration.
pub(crate) struct LoggingOptions {
    pub(crate) filter: EnvFilter,
    pub(crate) max_bytes: u64,
    pub(crate) retention: Retention,
    pub(crate) dev: bool,
}

/// What happens to a log file that reaches `max_bytes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retention {
    /// Shift into `name.1` .. `name.N`; with 0 the file is replaced.
    Generations(usize),
    /// Move into a timestamped archive that is gzipped off the logging path.
    Archive(ArchivePolicy),
}

/// Archives newer than `max_age` are kept unless together they pass
/// `max_total_bytes`, which then drops the oldest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArchivePolicy {
    pub(crate) max_age: Duration,
    pub(crate) max_total_bytes: u64,
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

pub(crate) fn startup(role: &'static str, options: &LoggingOptions) {
    tracing::info!(
        target: "bus::utils::logging",
        event = "app.startup",
        subsystem = role,
        outcome = "started",
        pid = std::process::id(),
        dev = options.dev,
        log_filter = %options.filter,
        log_max_bytes = options.max_bytes,
        log_retention = ?options.retention,
        version = env!("CARGO_PKG_VERSION"),
        build_id = ?crate::utils::version::build_id(),
        "bus starting"
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
            retention: options.retention,
            file: None,
            current_size: 0,
            disabled: false,
            archive_signal: None,
        };
        if let Retention::Archive(policy) = options.retention {
            state.archive_signal = Some(spawn_archive_worker(state.path.clone(), policy));
        }
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
    retention: Retention,
    file: Option<File>,
    current_size: u64,
    disabled: bool,
    /// Wakes the archive worker; it also runs once at startup for leftovers.
    archive_signal: Option<mpsc::Sender<()>>,
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
        let retained_files = match self.retention {
            Retention::Generations(count) => count,
            Retention::Archive(_) => {
                // Only a rename under the lock; compression runs on the worker.
                match fs::rename(&self.path, archive_path(&self.path, SystemTime::now())) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
                if let Some(signal) = &self.archive_signal {
                    let _ = signal.send(());
                }
                self.current_size = 0;
                return Ok(());
            }
        };
        if retained_files == 0 {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            self.current_size = 0;
            return Ok(());
        }

        let oldest = rotated_log_path(&self.path, retained_files);
        match fs::remove_file(&oldest) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        for index in (1..=retained_files).rev() {
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

fn spawn_archive_worker(path: PathBuf, policy: ArchivePolicy) -> mpsc::Sender<()> {
    let (signal, wake) = mpsc::channel();
    let _ = signal.send(());
    let _ = std::thread::Builder::new()
        .name("bus-log-archive".to_owned())
        .spawn(move || {
            // Never log here: this thread serves the log writer itself.
            while wake.recv().is_ok() {
                while wake.try_recv().is_ok() {}
                maintain_archives(&path, &policy, SystemTime::now());
            }
        });
    signal
}

const ARCHIVE_GZ: &str = ".gz";
const ARCHIVE_TMP: &str = ".gz.tmp";

/// Archive file name for a log rotated at `at`, before compression. The UTC
/// stamp sorts by rotation time, which is the archive's newest entry.
fn archive_path(path: &Path, at: SystemTime) -> PathBuf {
    let at = time::OffsetDateTime::from(at);
    let stamp = format!(
        ".{:04}{:02}{:02}T{:02}{:02}{:02}.{:09}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.nanosecond()
    );
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(stamp);
    path.with_file_name(name)
}

/// Parses the rotation time back out of an archive name stem.
fn archive_time(stem: &str) -> Option<SystemTime> {
    let bytes = stem.as_bytes();
    if bytes.len() != 26 || bytes[8] != b'T' || bytes[15] != b'.' || bytes[25] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| stem.get(range)?.parse::<u32>().ok();
    let date = time::Date::from_calendar_date(
        number(0..4)? as i32,
        time::Month::try_from(number(4..6)? as u8).ok()?,
        number(6..8)? as u8,
    )
    .ok()?;
    let clock = time::Time::from_hms_nano(
        number(9..11)? as u8,
        number(11..13)? as u8,
        number(13..15)? as u8,
        number(16..25)?,
    )
    .ok()?;
    Some(
        time::PrimitiveDateTime::new(date, clock)
            .assume_utc()
            .into(),
    )
}

struct Archive {
    path: PathBuf,
    rotated_at: SystemTime,
    compressed: bool,
}

/// Compresses archives left plain and drops those outside `policy`:
/// older than `max_age`, then the oldest past `max_total_bytes`.
fn maintain_archives(path: &Path, policy: &ArchivePolicy, now: SystemTime) {
    let (Some(dir), Some(base)) = (path.parent(), path.file_name()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{}.", base.to_string_lossy());
    let mut archives = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if rest.ends_with(ARCHIVE_TMP) {
            // Interrupted compression; its plain archive is still there.
            let _ = fs::remove_file(entry.path());
            continue;
        }
        let compressed = rest.ends_with(ARCHIVE_GZ);
        let stem = rest.strip_suffix(ARCHIVE_GZ).unwrap_or(rest);
        if let Some(rotated_at) = archive_time(stem) {
            archives.push(Archive {
                path: entry.path(),
                rotated_at,
                compressed,
            });
        }
    }
    for archive in archives.iter_mut().filter(|archive| !archive.compressed) {
        if let Ok(compressed) = compress_archive(&archive.path) {
            archive.path = compressed;
            archive.compressed = true;
        }
    }

    archives.sort_by_key(|archive| std::cmp::Reverse(archive.rotated_at));
    let mut total: u64 = 0;
    for archive in archives {
        let expired = now
            .duration_since(archive.rotated_at)
            .is_ok_and(|age| age > policy.max_age);
        let size = fs::metadata(&archive.path).map_or(0, |meta| meta.len());
        total = total.saturating_add(size);
        if expired || total > policy.max_total_bytes {
            let _ = fs::remove_file(&archive.path);
        }
    }
}

fn compress_archive(plain: &Path) -> io::Result<PathBuf> {
    let mut name = plain.as_os_str().to_os_string();
    name.push(ARCHIVE_TMP);
    let temporary = PathBuf::from(name);
    let compressed = temporary.with_extension("");
    let mut encoder =
        flate2::write::GzEncoder::new(File::create(&temporary)?, flate2::Compression::default());
    io::copy(&mut File::open(plain)?, &mut encoder)?;
    encoder.finish()?.sync_all()?;
    fs::rename(&temporary, &compressed)?;
    fs::remove_file(plain)?;
    Ok(compressed)
}

#[cfg(test)]
#[path = "tests/logging_test.rs"]
mod tests;
