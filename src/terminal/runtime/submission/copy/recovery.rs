//! Persist copied drafts without holding the pane input lease across filesystem I/O.
use std::{
    collections::HashMap,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Mutex, MutexGuard, OnceLock},
    thread,
};

const MAX_RECORDS: usize = 16;
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const QUEUED_BACKUPS: usize = 16;

#[derive(Clone)]
pub(super) enum Outcome {
    Restored,
    Retain { reason: &'static str, error: String },
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct Key {
    directory: PathBuf,
    digest: String,
}

#[derive(Clone)]
struct Context {
    key: Key,
    span: tracing::Span,
    dispatch: tracing::Dispatch,
}

enum Work {
    Save(Context, String),
    Settle(Context, u64),
}

#[derive(Default)]
struct Entry {
    generation: u64,
    active: usize,
    saving: usize,
    controls: usize,
    outcome: Option<Outcome>,
    applied: bool,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<Key, Entry>,
    next_generation: u64,
    queued: usize,
}

impl Registry {
    fn discard_idle(&mut self, key: &Key) {
        if self
            .entries
            .get(key)
            .is_some_and(|state| state.active == 0 && state.saving == 0 && state.controls == 0)
        {
            self.entries.remove(key);
        }
    }
}

#[derive(Clone)]
struct Service {
    work: mpsc::Sender<Work>,
    registry: std::sync::Arc<Mutex<Registry>>,
}

impl Service {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn run(&self, receive: mpsc::Receiver<Work>) {
        for work in receive {
            let context = match &work {
                Work::Save(context, _) | Work::Settle(context, _) => context.clone(),
            };
            tracing::dispatcher::with_default(&context.dispatch, || {
                let _span = context.span.enter();
                match work {
                    Work::Save(_, snapshot) => self.persist(&context.key, snapshot),
                    Work::Settle(_, generation) => self.settle(&context.key, generation),
                }
            });
        }
    }

    fn persist(&self, key: &Key, snapshot: String) {
        let should_save = {
            let mut registry = self.registry();
            registry.queued -= 1;
            registry.entries.get(key).is_some_and(|state| {
                state.active != 0 || !matches!(state.outcome, Some(Outcome::Restored))
            })
        };
        if should_save {
            match save(key, snapshot.as_bytes()) {
                Ok(path) => self.enforce_retention(key, &path),
                Err(error) => {
                    tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_storage_failed", %error, "Could not save a copied draft recovery record")
                }
            }
        }
        let mut registry = self.registry();
        if let Some(state) = registry.entries.get_mut(key) {
            state.saving -= 1;
        }
        registry.discard_idle(key);
    }

    fn settle(&self, key: &Key, generation: u64) {
        // Lookups can block on storage. Recheck the outcome after lookup
        // and never hold the registry across filesystem operations.
        if self.current(key, generation) {
            let backups = verified_records(key);
            let outcome = self.registry().entries.get_mut(key).and_then(|state| {
                if state.active != 0 || state.generation != generation || state.applied {
                    return None;
                }
                state.applied = true;
                state.outcome.clone()
            });
            if let Some(outcome) = outcome {
                self.apply_outcome(key, generation, outcome, backups);
            }
        }
        let mut registry = self.registry();
        if let Some(state) = registry.entries.get_mut(key) {
            state.controls -= 1;
        }
        registry.discard_idle(key);
    }

    fn current(&self, key: &Key, generation: u64) -> bool {
        self.registry().entries.get(key).is_some_and(|state| {
            state.active == 0 && state.generation == generation && state.outcome.is_some()
        })
    }

    fn apply_outcome(
        &self,
        key: &Key,
        generation: u64,
        outcome: Outcome,
        backups: io::Result<Vec<PathBuf>>,
    ) {
        let backups = match backups {
            Ok(backups) => backups,
            Err(error) => {
                tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_lookup_failed", %error, "Could not verify copied draft recovery records");
                Vec::new()
            }
        };
        match outcome {
            Outcome::Restored => {
                if let Some(path) = backups.first() {
                    self.enforce_retention(key, path);
                }
                for path in backups {
                    if !self.current(key, generation) {
                        return;
                    }
                    if let Err(error) = fs::remove_file(&path) {
                        tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_retirement_failed", %error, "Could not retire a restored copy");
                        return;
                    }
                }
                tracing::debug!(
                    event = "bus.draft.copy_recovery_retired",
                    "Verified restore has no remaining recovery record"
                );
            }
            Outcome::Retain { reason, error } => {
                if let Some(path) = backups.first() {
                    self.enforce_retention(key, path);
                    // Pruning can take time. A newer attempt owns any notice.
                    if !self.current(key, generation) {
                        return;
                    }
                    tracing::warn!(event = "bus.draft.recovery_required", reason, recovery = %path.display(), %error, "Copied composer snapshot retained for manual recovery");
                } else if self.current(key, generation) {
                    tracing::warn!(event = "bus.draft.copy_loss", reason = "snapshot_storage_unavailable", draft_outcome = reason, %error, "Copied draft restoration was not proven and no verified recovery record is available");
                }
            }
        }
    }

    fn enforce_retention(&self, key: &Key, current: &Path) {
        if let Err(error) = prune(&key.directory, current, |digest| {
            let candidate = Key {
                directory: key.directory.clone(),
                digest,
            };
            self.registry()
                .entries
                .get(&candidate)
                .is_some_and(|state| state.active != 0 || state.saving != 0 || state.controls != 0)
        }) {
            tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_prune_failed", %error, "Could not prune copied draft recovery records");
        }
    }
}

pub(super) struct Attempt {
    service: &'static Service,
    context: Option<Context>,
    generation: u64,
}

impl Attempt {
    pub(super) fn send(mut self, outcome: Outcome) {
        self.finish(outcome);
    }

    fn finish(&mut self, outcome: Outcome) {
        let Some(context) = self.context.take() else {
            return;
        };
        let mut registry = self.service.registry();
        let Some(entry) = registry.entries.get_mut(&context.key) else {
            return;
        };
        entry.active -= 1;
        if entry.generation == self.generation {
            entry.outcome = Some(outcome);
        }
        // The control has no draft bytes and is independent of snapshot capacity.
        entry.controls += 1;
        let generation = entry.generation;
        if self
            .service
            .work
            .send(Work::Settle(context, generation))
            .is_err()
        {
            tracing::warn!(
                event = "bus.draft.copy_recovery_failed",
                reason = "snapshot_worker_unavailable",
                "Could not settle copied draft recovery"
            );
        }
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.finish(Outcome::Retain {
            reason: "attempt_unsettled",
            error: "Draft attempt ended without a restore verdict".into(),
        });
    }
}

pub(super) fn start(directory: &Path, snapshot: &str) -> Option<Attempt> {
    if snapshot.is_empty() {
        return None;
    }
    static WORKER: OnceLock<Option<Service>> = OnceLock::new();
    let service = WORKER.get_or_init(|| {
        let (work, receive) = mpsc::channel();
        let service = Service { work, registry: Default::default() };
        let worker = service.clone();
        thread::Builder::new()
            .name("copy-draft-recovery".into())
            .spawn(move || worker.run(receive))
            .map(|_| service)
            .inspect_err(|error| {
                tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_worker_unavailable", %error, "Could not start copied draft storage");
            })
            .ok()
    }).as_ref()?;
    let key = Key {
        directory: directory.to_owned(),
        digest: crate::utils::time::digest(snapshot.as_bytes()),
    };
    let context = Context {
        key,
        span: tracing::Span::current(),
        dispatch: tracing::dispatcher::get_default(Clone::clone),
    };
    let (generation, skipped_reason) = register(service, &context, snapshot);
    if let Some(reason) = skipped_reason {
        tracing::warn!(
            event = "bus.draft.copy_recovery_failed",
            reason,
            "Copied draft persistence was not queued"
        );
    }
    Some(Attempt {
        service,
        generation,
        context: Some(context),
    })
}

fn register(service: &Service, context: &Context, snapshot: &str) -> (u64, Option<&'static str>) {
    let mut registry = service.registry();
    registry.next_generation += 1;
    let generation = registry.next_generation;
    let full = registry.queued >= QUEUED_BACKUPS;
    let entry = registry.entries.entry(context.key.clone()).or_default();
    entry.generation = generation;
    entry.active += 1;
    entry.outcome = None;
    entry.applied = false;
    let reason = if snapshot.len() as u64 > crate::platform::draft_editor::MAX_BYTES {
        Some("snapshot_too_large")
    } else if full {
        Some("snapshot_worker_busy")
    } else if service
        .work
        .send(Work::Save(context.clone(), snapshot.to_owned()))
        .is_err()
    {
        Some("snapshot_worker_unavailable")
    } else {
        // Register and enqueue together so a concurrent terminal control
        // cannot overtake a save whose capacity has already been reserved.
        entry.saving += 1;
        registry.queued += 1;
        None
    };
    (generation, reason)
}

fn save(key: &Key, snapshot: &[u8]) -> io::Result<PathBuf> {
    let Key { directory, digest } = key;
    crate::platform::fs::private_dir(directory)?;
    let records = records(directory)?;
    if let Some(record) = records
        .iter()
        .find(|record| record.digest == *digest && matches_snapshot(&record.path, snapshot))
    {
        return Ok(record.path.clone());
    }

    #[cfg(test)]
    test_hooks::before_write(directory)?;
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
    read_private(path).is_some_and(|bytes| bytes == snapshot)
}

fn verified_records(key: &Key) -> io::Result<Vec<PathBuf>> {
    let records = match records(&key.directory) {
        Ok(records) => records,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(records
        .into_iter()
        .filter(|record| {
            record.digest == key.digest
                && read_private(&record.path)
                    .is_some_and(|bytes| crate::utils::time::digest(&bytes) == key.digest)
        })
        .map(|record| record.path)
        .collect())
}

fn read_private(path: &Path) -> Option<Vec<u8>> {
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
    let file = fs::symlink_metadata(path)
        .ok()
        .filter(private_file)
        .and_then(|_| open(path).ok())?;
    let metadata = file.metadata().ok().filter(private_file)?;
    let size = metadata.len();
    if size > crate::platform::draft_editor::MAX_BYTES {
        return None;
    }
    let mut contents = Vec::with_capacity(size as usize);
    file.take(size + 1).read_to_end(&mut contents).ok()?;
    (contents.len() as u64 == size).then_some(contents)
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
    directory: &Path,
    current: &Path,
    is_protected: impl Fn(String) -> bool,
) -> io::Result<()> {
    let mut records = records(directory)?;
    let mut count = records.len();
    let mut bytes = records.iter().fold(0_u64, |sum, record| {
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
            break;
        }
        // Authorize eviction only after replacement is durable, against
        // current protection rather than the earlier directory scan.
        if record.path != current && !is_protected(record.digest) {
            fs::remove_file(&record.path)?;
            count -= 1;
            bytes = bytes.saturating_sub(record.metadata.len());
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    type Action = Box<dyn FnOnce() -> io::Result<()> + Send>;
    static WRITES: OnceLock<Mutex<HashMap<PathBuf, Action>>> = OnceLock::new();

    pub(crate) fn install(
        directory: PathBuf,
        action: impl FnOnce() -> io::Result<()> + Send + 'static,
    ) {
        let previous = WRITES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(directory, Box::new(action));
        assert!(previous.is_none());
    }

    pub(super) fn before_write(directory: &Path) -> io::Result<()> {
        let action = WRITES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(directory);
        action.map_or(Ok(()), |action| action())
    }

    pub(crate) fn retain(directory: &Path, text: &str) {
        if let Some(attempt) = start(directory, text) {
            attempt.send(Outcome::Retain {
                reason: "test_retain",
                error: String::new(),
            });
        }
    }
}
