//! Persist copied drafts without holding the pane input lease across filesystem I/O.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{mpsc, OnceLock},
    thread,
};

const MAX_RECORDS: usize = 16;
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const QUEUED_BACKUPS: usize = 16;

pub(super) enum Outcome {
    Restored,
    Retain { reason: &'static str, error: String },
}

struct Job {
    directory: PathBuf,
    snapshot: String,
    outcome: mpsc::Receiver<Outcome>,
    span: tracing::Span,
    dispatch: tracing::Dispatch,
}

pub(super) fn start(directory: &Path, snapshot: &str) -> Option<mpsc::Sender<Outcome>> {
    if snapshot.is_empty() {
        return None;
    }
    if snapshot.len() as u64 > crate::platform::draft_editor::MAX_BYTES {
        tracing::warn!(
            event = "bus.draft.copy_recovery_failed",
            reason = "snapshot_too_large",
            "Copied draft exceeds the recovery record size limit"
        );
        return None;
    }
    static WORKER: OnceLock<Option<mpsc::SyncSender<Job>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel::<Job>(QUEUED_BACKUPS);
        thread::Builder::new()
            .name("copy-draft-recovery".into())
            .spawn(move || {
                for job in receive {
                    tracing::dispatcher::with_default(&job.dispatch, || {
                        let _span = job.span.enter();
                        job.persist();
                    });
                }
            })
            .map(|_| send)
            .inspect_err(|error| {
                tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_worker_unavailable", %error, "Could not start copied draft storage");
            })
            .ok()
    }).as_ref()?;
    let (send, outcome) = mpsc::channel();
    let job = Job {
        directory: directory.to_owned(),
        snapshot: snapshot.to_owned(),
        outcome,
        span: tracing::Span::current(),
        dispatch: tracing::dispatcher::get_default(Clone::clone),
    };
    match worker.try_send(job) {
        Ok(()) => Some(send),
        Err(error) => {
            // Do not format the queued job. It owns the snapshot text.
            let reason = match error {
                mpsc::TrySendError::Full(_) => "snapshot_worker_busy",
                mpsc::TrySendError::Disconnected(_) => "snapshot_worker_unavailable",
            };
            tracing::warn!(
                event = "bus.draft.copy_recovery_failed",
                reason,
                "Copied draft storage is unavailable"
            );
            None
        }
    }
}

impl Job {
    fn persist(&self) {
        let backup = save(&self.directory, self.snapshot.as_bytes());
        if let Err(error) = &backup {
            tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_storage_failed", %error, "Could not save a copied draft recovery record");
        }
        let outcome = self.outcome.recv().unwrap_or_else(|_| Outcome::Retain {
            reason: "attempt_unsettled",
            error: "Draft attempt ended without a restore verdict".into(),
        });
        let Ok(path) = backup else {
            if let Outcome::Retain { reason, error } = outcome {
                tracing::warn!(event = "bus.draft.copy_loss", reason = "snapshot_storage_unavailable", draft_outcome = reason, %error, "Copied draft restoration was not proven and recovery storage failed");
            }
            return;
        };
        if !matches_snapshot(&path, self.snapshot.as_bytes()) {
            tracing::warn!(
                event = "bus.draft.copy_recovery_failed",
                reason = "snapshot_changed",
                "Copied draft recovery record was replaced or edited"
            );
            return;
        }
        match outcome {
            Outcome::Restored => match fs::remove_file(&path) {
                Ok(()) => tracing::debug!(
                    event = "bus.draft.copy_recovery_retired",
                    "Verified restoration retired the recovery record"
                ),
                Err(error) => {
                    tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_retirement_failed", %error, "Could not retire a restored copy")
                }
            },
            Outcome::Retain { reason, error } => {
                tracing::warn!(event = "bus.draft.recovery_required", reason, recovery = %path.display(), %error, "Copied composer snapshot retained for manual recovery");
            }
        }
    }
}

fn save(directory: &Path, snapshot: &[u8]) -> io::Result<PathBuf> {
    crate::platform::fs::private_dir(directory)?;
    let digest = crate::utils::time::digest(snapshot);
    let records = records(directory)?;
    if let Some(record) = records
        .iter()
        .find(|record| record.digest == digest && matches_snapshot(&record.path, snapshot))
    {
        let path = record.path.clone();
        prune(records, Some(&path), 0, 0)?;
        return Ok(path);
    }

    prune(records, None, 1, snapshot.len() as u64)?;
    let primary = directory.join(format!("copied-{digest}.txt"));
    let (path, mut file) = match crate::platform::create_private_state_file(&primary) {
        Ok(file) => (primary, file),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                || fs::symlink_metadata(&primary).is_ok() =>
        {
            let nonce = crate::utils::time::now_ns();
            let alternate = directory.join(format!(
                "copied-{digest}-{}-{nonce}.txt",
                std::process::id()
            ));
            let file = crate::platform::create_private_state_file(&alternate)?;
            (alternate, file)
        }
        Err(error) => return Err(error),
    };
    let written = file.write_all(snapshot).and_then(|()| file.sync_all());
    drop(file);
    let published = written
        .and_then(|()| crate::platform::sync_parent_directory(directory))
        .and_then(|()| {
            matches_snapshot(&path, snapshot)
                .then_some(())
                .ok_or_else(|| io::Error::other("Copied draft could not be read back"))
        });
    if let Err(error) = published {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn matches_snapshot(path: &Path, snapshot: &[u8]) -> bool {
    fn open(path: &Path) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        options.open(path)
    }
    let Some(file) = fs::symlink_metadata(path)
        .ok()
        .filter(private_file)
        .and_then(|_| open(path).ok())
    else {
        return false;
    };
    let size = snapshot.len() as u64;
    if !file
        .metadata()
        .is_ok_and(|metadata| private_file(&metadata) && metadata.len() == size)
    {
        return false;
    }
    let mut contents = Vec::with_capacity(snapshot.len());
    file.take(size + 1).read_to_end(&mut contents).is_ok() && contents == snapshot
}

fn private_file(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return false;
        }
    }
    metadata.file_type().is_file()
}

struct Record {
    path: PathBuf,
    digest: String,
    metadata: Metadata,
}

fn records(directory: &Path) -> io::Result<Vec<Record>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((digest, suffix)) = name
            .to_str()
            .and_then(|name| name.strip_prefix("copied-")?.strip_suffix(".txt"))
            .map(|stem| stem.split_once('-').unwrap_or((stem, "")))
        else {
            continue;
        };
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-')
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if private_file(&metadata) {
            records.push(Record {
                path,
                digest: digest.to_owned(),
                metadata,
            });
        }
    }
    Ok(records)
}

fn prune(
    mut records: Vec<Record>,
    keep: Option<&Path>,
    extra_count: usize,
    extra_bytes: u64,
) -> io::Result<()> {
    let mut count = records.len().saturating_add(extra_count);
    let mut bytes = records.iter().fold(extra_bytes, |sum, record| {
        sum.saturating_add(record.metadata.len())
    });
    records.sort_unstable_by(|a, b| {
        a.metadata
            .modified()
            .ok()
            .cmp(&b.metadata.modified().ok())
            .then_with(|| a.path.cmp(&b.path))
    });
    for record in records {
        if count <= MAX_RECORDS && bytes <= MAX_BYTES {
            return Ok(());
        }
        if keep != Some(record.path.as_path()) {
            fs::remove_file(&record.path)?;
            count -= 1;
            bytes -= record.metadata.len();
        }
    }
    Ok(())
}
