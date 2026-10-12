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

    /// Runs `action` under `options` writing through the real rotating file
    /// writer at `dir/file_name`, scoped to this thread.
    pub(crate) fn run_to_file(
        dir: std::path::PathBuf,
        file_name: &str,
        options: &super::LoggingOptions,
        action: impl FnOnce(),
    ) {
        let writer = super::RotatingFileMakeWriter::new(dir, file_name, options).unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(options.filter.clone())
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, action);
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
        // Held across check, rotation and append so no other process sharing
        // this log renames it or appends to it in between.
        let Ok(_shared) = state.lock_for_append(buf.len() as u64, SharedLogLock(None)) else {
            state.disabled = true;
            return Ok(buf.len());
        };
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

/// An exclusive lock on the live log file itself, shared by every process
/// that writes it; released on drop.
struct SharedLogLock(Option<File>);

impl SharedLogLock {
    /// Whether this is a held lock on `file`'s inode; no lock covers nothing.
    #[cfg(unix)]
    fn covers(&self, file: Option<&File>) -> bool {
        use std::os::unix::fs::MetadataExt;
        let (Some(lock), Some(file)) = (&self.0, file) else {
            return false;
        };
        match (lock.metadata(), file.metadata()) {
            (Ok(lock), Ok(file)) => (lock.dev(), lock.ino()) == (file.dev(), file.ino()),
            _ => false,
        }
    }

    /// Without inode identity, a held lock is on the file it was cloned from.
    #[cfg(not(unix))]
    fn covers(&self, file: Option<&File>) -> bool {
        self.0.is_some() && file.is_some()
    }
}

impl Drop for SharedLogLock {
    fn drop(&mut self) {
        if let Some(lock) = &self.0 {
            let _ = lock.unlock();
        }
    }
}

impl RotatingFileState {
    /// Blocks until this process holds the lock of the file that is live at
    /// the log path, following renames by other writers, and counts the bytes
    /// they appended. Errors only on I/O failure, never with the file unlocked.
    fn lock_live_file(&mut self) -> io::Result<SharedLogLock> {
        loop {
            if self.file.is_none() || self.rotated_by_another_writer() {
                self.open_current_file()?;
            }
            let lock = self
                .file
                .as_ref()
                .ok_or_else(|| io::Error::other("log file is not open"))?
                .try_clone()?;
            lock.lock()?;
            let lock = SharedLogLock(Some(lock));
            // Another writer may have renamed the file while this one waited.
            if !self.rotated_by_another_writer() {
                if let Some(file) = self.file.as_ref() {
                    self.current_size = file.metadata()?.len();
                }
                return Ok(lock);
            }
        }
    }

    /// Returns the permission to append `incoming_len` bytes. In archive mode
    /// that is the lock of the live file, granted only once its refreshed size
    /// admits the entry (or it is empty), rotating under the lock as needed:
    /// starting from `held`, it relocks until both hold. Other writers'
    /// rotations are the only reason to loop, so it ends once one admits it.
    fn lock_for_append(
        &mut self,
        incoming_len: u64,
        mut held: SharedLogLock,
    ) -> io::Result<SharedLogLock> {
        if !matches!(self.retention, Retention::Archive(_)) {
            self.rotate_if_needed(incoming_len)?;
            return Ok(held);
        }
        loop {
            if !held.covers(self.file.as_ref()) || self.rotated_by_another_writer() {
                drop(held);
                held = self.lock_live_file()?;
            }
            if self.current_size == 0
                || self.current_size.saturating_add(incoming_len) <= self.max_bytes
            {
                return Ok(held);
            }
            self.rotate_files()?;
            self.open_current_file()?;
        }
    }

    fn rotate_if_needed(&mut self, incoming_len: u64) -> io::Result<()> {
        if self.file.is_none() || self.rotated_by_another_writer() {
            self.open_current_file()?;
        }
        // An empty file is never rotated, even for an entry over the limit.
        if self.current_size == 0
            || self.current_size.saturating_add(incoming_len) <= self.max_bytes
        {
            return Ok(());
        }
        self.rotate_files()?;
        self.open_current_file()
    }

    /// Another process sharing this log (a second `bus client`) renamed it into
    /// an archive; keep writing to the live path, not into that archive.
    #[cfg(unix)]
    fn rotated_by_another_writer(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        let (Retention::Archive(_), Some(file)) = (self.retention, self.file.as_ref()) else {
            return false;
        };
        let (Ok(open), path) = (file.metadata(), fs::metadata(&self.path)) else {
            return false;
        };
        !path.is_ok_and(|path| (path.dev(), path.ino()) == (open.dev(), open.ino()))
    }

    #[cfg(not(unix))]
    fn rotated_by_another_writer(&self) -> bool {
        false
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
            Retention::Archive(policy) => {
                // Rename and enforce the cap under the lock, so the cap holds
                // however far behind compression on the worker is.
                let now = SystemTime::now();
                match fs::rename(&self.path, archive_path(&self.path, now)) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
                prune_archives(list_archives(&self.path), &policy, now);
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
const ARCHIVE_TMP: &str = ".tmp";

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
    remove_interrupted_compressions(path);
    let mut archives = list_archives(path);
    for archive in archives.iter_mut().filter(|archive| !archive.compressed) {
        if let Ok(compressed) = compress_archive(&archive.path) {
            archive.path = compressed;
            archive.compressed = true;
        }
    }
    prune_archives(archives, policy, now);
}

/// Archive names under `path`'s directory, one entry per archive. A plain
/// copy left beside its finished `.gz` (a process stopped between the
/// rename and the removal) is removed rather than counted twice.
fn list_archives(path: &Path) -> Vec<Archive> {
    let mut archives: Vec<Archive> = Vec::new();
    let mut plain = Vec::new();
    for (entry, rest) in archive_dir_entries(path) {
        let compressed = rest.ends_with(ARCHIVE_GZ);
        let stem = rest.strip_suffix(ARCHIVE_GZ).unwrap_or(&rest);
        if let Some(rotated_at) = archive_time(stem) {
            let archive = Archive {
                path: entry,
                rotated_at,
                compressed,
            };
            if compressed {
                archives.push(archive);
            } else {
                plain.push(archive);
            }
        }
    }
    for archive in plain {
        let mut finished = archive.path.clone().into_os_string();
        finished.push(ARCHIVE_GZ);
        if archives.iter().any(|gz| gz.path.as_os_str() == finished) {
            let _ = fs::remove_file(&archive.path);
        } else {
            archives.push(archive);
        }
    }
    archives
}

/// Deletes `.gz[.<writer>].tmp` staging files of Bus archives only; other
/// names stay.
fn remove_interrupted_compressions(path: &Path) {
    for (entry, rest) in archive_dir_entries(path) {
        let stem = rest.strip_suffix(ARCHIVE_TMP).and_then(|staged| {
            staged.strip_suffix(ARCHIVE_GZ).or_else(|| {
                let (stem, writer) = staged.split_once(".gz.")?;
                let (pid, sequence) = writer.split_once('-')?;
                let numeric =
                    |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
                (numeric(pid) && numeric(sequence)).then_some(stem)
            })
        });
        if stem.is_some_and(|stem| archive_time(stem).is_some()) {
            let _ = fs::remove_file(entry);
        }
    }
}

/// Entries named `<log name>.<rest>` beside `path`, with `rest`.
fn archive_dir_entries(path: &Path) -> Vec<(PathBuf, String)> {
    let (Some(dir), Some(base)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = format!("{}.", base.to_string_lossy());
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix(&prefix)?.to_owned();
            Some((entry.path(), rest))
        })
        .collect()
}

fn prune_archives(mut archives: Vec<Archive>, policy: &ArchivePolicy, now: SystemTime) {
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

/// Gzips `plain` into `<plain>.gz` through a staging file of its own: workers
/// of several processes may compress the same archive at once, and a shared
/// staging name let one truncate the other's finished output.
fn compress_archive(plain: &Path) -> io::Result<PathBuf> {
    static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut compressed = plain.as_os_str().to_os_string();
    compressed.push(ARCHIVE_GZ);
    let compressed = PathBuf::from(compressed);
    let mut staging = compressed.clone().into_os_string();
    let sequence = STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    staging.push(format!(".{}-{sequence}{ARCHIVE_TMP}", std::process::id()));
    let staging = PathBuf::from(staging);
    let source = File::open(plain)?;
    let staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    let result = (|| {
        let mut encoder = flate2::write::GzEncoder::new(staged, flate2::Compression::default());
        io::copy(&mut &source, &mut encoder)?;
        encoder.finish()?.sync_all()?;
        fs::rename(&staging, &compressed)
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&staging);
        return Err(err);
    }
    match fs::remove_file(plain) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    Ok(compressed)
}

#[cfg(test)]
#[path = "tests/logging_test.rs"]
mod tests;
