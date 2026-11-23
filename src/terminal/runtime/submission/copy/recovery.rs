//! Persist copied drafts without holding the pane input lease across storage I/O.
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
const MAX_TRACKED: usize = 256;

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
    Settle(Key),
    Revisit,
}

struct Resolution {
    context: Context,
    generation: u64,
    outcome: Outcome,
}

#[derive(Default)]
struct Entry {
    generation: u64,
    active: usize,
    saving: usize,
    control_queued: bool,
    settling: bool,
    restored: bool,
    resolution: Option<Resolution>,
}

impl Entry {
    fn protected(&self) -> bool {
        self.active != 0 || self.saving != 0 || self.control_queued || self.settling
    }
}

#[derive(Default)]
struct Registry {
    entries: HashMap<Key, Entry>,
    deferred: HashMap<PathBuf, PathBuf>,
    next_generation: u64,
    queued: usize,
    overflow: usize,
    revisit_queued: bool,
    disconnected: bool,
}

impl Registry {
    fn discard_idle(&mut self, key: &Key) {
        if self
            .entries
            .get(key)
            .is_some_and(|state| !state.protected())
        {
            self.entries.remove(key);
        }
    }

    fn current(&self, key: &Key, generation: u64) -> bool {
        self.entries
            .get(key)
            .is_some_and(|state| state.active == 0 && state.generation == generation)
    }

    fn disconnect(&mut self) {
        *self = Self {
            disconnected: true,
            ..Self::default()
        };
    }
}

#[derive(Clone)]
struct Service {
    work: mpsc::Sender<Work>,
    registry: std::sync::Arc<Mutex<Registry>>,
}

struct WorkerLifetime<'a>(&'a Service);

impl Drop for WorkerLifetime<'_> {
    fn drop(&mut self) {
        self.0.registry().disconnect();
    }
}

impl Service {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn run(&self, receive: mpsc::Receiver<Work>) {
        let _lifetime = WorkerLifetime(self);
        for work in receive {
            match work {
                Work::Save(context, snapshot) => {
                    tracing::dispatcher::with_default(&context.dispatch, || {
                        let _span = context.span.enter();
                        self.persist(&context.key, snapshot);
                    });
                }
                Work::Settle(key) => self.settle(&key),
                Work::Revisit => {
                    let directories = {
                        let mut registry = self.registry();
                        registry.revisit_queued = false;
                        registry.deferred.keys().cloned().collect::<Vec<_>>()
                    };
                    for directory in directories {
                        self.revisit_retention(&directory);
                    }
                }
            }
        }
    }

    fn persist(&self, key: &Key, snapshot: String) {
        let should_save = {
            let mut registry = self.registry();
            registry.queued -= 1;
            registry
                .entries
                .get(key)
                .is_some_and(|state| state.active != 0 || !state.restored)
        };
        if should_save {
            match save(key, snapshot.as_bytes()) {
                Ok(path) => self.enforce_retention(key, &path),
                Err(error) => {
                    tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_storage_failed", %error, "Could not save a copied draft recovery record")
                }
            }
        }
        {
            let mut registry = self.registry();
            if let Some(state) = registry.entries.get_mut(key) {
                state.saving -= 1;
            }
            registry.discard_idle(key);
        }
        self.revisit_retention(&key.directory);
    }

    fn settle(&self, key: &Key) {
        let resolution = {
            let mut registry = self.registry();
            let Some(state) = registry.entries.get_mut(key) else {
                return;
            };
            state.control_queued = false;
            state.settling = true;
            if state.active == 0 {
                state.resolution.take()
            } else {
                None
            }
        };
        if let Some(resolution) = resolution {
            tracing::dispatcher::with_default(&resolution.context.dispatch, || {
                let _span = resolution.context.span.enter();
                let backups = verified_records(key).unwrap_or_else(|error| {
                    tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_lookup_failed", %error, "Could not verify copied draft recovery records");
                    Vec::new()
                });
                self.apply_outcome(key, resolution.generation, resolution.outcome, backups);
            });
        }
        {
            let mut registry = self.registry();
            if let Some(state) = registry.entries.get_mut(key) {
                state.settling = false;
            }
            registry.discard_idle(key);
        }
        // Protection can end even when this digest has no verified file.
        self.revisit_retention(&key.directory);
    }

    fn apply_outcome(&self, key: &Key, generation: u64, outcome: Outcome, backups: Vec<PathBuf>) {
        if let Some(path) = backups.first() {
            if self.registry().current(key, generation) {
                self.enforce_retention(key, path);
            }
        }
        match outcome {
            Outcome::Restored => {
                for path in backups {
                    match discard_if(&path, &key.digest, || {
                        self.registry().current(key, generation)
                    }) {
                        Ok(true) => {}
                        Ok(false) => return,
                        Err(error) => {
                            tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_retirement_failed", %error, "Could not retire a restored copy");
                            return;
                        }
                    }
                }
                if self.registry().current(key, generation) {
                    tracing::debug!(
                        event = "bus.draft.copy_recovery_retired",
                        "Verified restore has no remaining recovery record"
                    );
                }
            }
            Outcome::Retain { reason, error } => {
                #[cfg(test)]
                if let Some(path) = backups.first() {
                    test_hooks::before_effect(path);
                }
                // Commit the structured notice and its generation check together.
                // File reads, sync, staging and unlink all stay outside this guard.
                let registry = self.registry();
                if !registry.current(key, generation) {
                    return;
                }
                if let Some(path) = backups.first() {
                    tracing::warn!(event = "bus.draft.recovery_required", reason, recovery = %path.display(), %error, "Copied composer snapshot retained for manual recovery");
                } else {
                    tracing::warn!(event = "bus.draft.copy_loss", reason = "snapshot_storage_unavailable", draft_outcome = reason, %error, "Copied draft restoration was not proven and no verified recovery record is available");
                }
            }
        }
    }

    fn enforce_retention(&self, key: &Key, current: &Path) {
        self.prune_directory(&key.directory, current);
    }

    fn revisit_retention(&self, directory: &Path) {
        let current = self.registry().deferred.get(directory).cloned();
        if let Some(current) = current {
            self.prune_directory(directory, &current);
        }
    }

    fn prune_directory(&self, directory: &Path, current: &Path) {
        let result = prune(directory, current, |digest| {
            let candidate = Key {
                directory: directory.to_owned(),
                digest: digest.to_owned(),
            };
            let registry = self.registry();
            registry.overflow == 0
                && !registry
                    .entries
                    .get(&candidate)
                    .is_some_and(Entry::protected)
        });
        match result {
            Ok(true) => {
                self.registry().deferred.remove(directory);
            }
            Ok(false) => {
                self.registry()
                    .deferred
                    .insert(directory.to_owned(), current.to_owned());
            }
            Err(error) => {
                // An inaccessible directory with no remaining protections
                // cannot be revisited by a later release. A future successful
                // save will run retention afresh.
                let mut registry = self.registry();
                if registry.overflow == 0
                    && !registry
                        .entries
                        .keys()
                        .any(|key| key.directory == directory)
                {
                    registry.deferred.remove(directory);
                }
                drop(registry);
                tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_prune_failed", %error, "Could not prune copied draft recovery records");
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Registration {
    Tracked(u64),
    Overflow,
    Unavailable,
}

pub(super) struct Attempt {
    service: &'static Service,
    context: Option<Context>,
    registration: Registration,
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
        if registry.disconnected {
            return;
        }
        let work = match self.registration {
            Registration::Tracked(generation) => {
                finish_tracked(&mut registry, context, generation, outcome)
            }
            Registration::Overflow => {
                registry.overflow -= 1;
                let wake = registry.overflow == 0 && !registry.revisit_queued;
                registry.revisit_queued |= wake;
                wake.then_some(Work::Revisit)
            }
            Registration::Unavailable => None,
        };
        if work.is_some_and(|work| self.service.work.send(work).is_err()) {
            registry.disconnect();
            drop(registry);
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

fn finish_tracked(
    registry: &mut Registry,
    context: Context,
    generation: u64,
    outcome: Outcome,
) -> Option<Work> {
    let key = context.key.clone();
    let entry = registry.entries.get_mut(&key)?;
    entry.active -= 1;
    if entry.generation == generation {
        entry.restored = matches!(outcome, Outcome::Restored);
        entry.resolution = Some(Resolution {
            context,
            generation,
            outcome,
        });
    }
    if entry.active != 0 || entry.control_queued {
        return None;
    }
    entry.control_queued = true;
    Some(Work::Settle(key))
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
    let context = Context {
        key: Key {
            directory: directory.to_owned(),
            digest: crate::utils::time::digest(snapshot.as_bytes()),
        },
        span: tracing::Span::current(),
        dispatch: tracing::dispatcher::get_default(Clone::clone),
    };
    let (registration, skipped_reason) = register(service, &context, snapshot);
    if let Some(reason) = skipped_reason {
        tracing::warn!(
            event = "bus.draft.copy_recovery_failed",
            reason,
            "Copied draft persistence was not queued"
        );
    }
    if matches!(registration, Registration::Unavailable) {
        return None;
    }
    Some(Attempt {
        service,
        registration,
        context: Some(context),
    })
}

fn register(
    service: &Service,
    context: &Context,
    snapshot: &str,
) -> (Registration, Option<&'static str>) {
    let mut registry = service.registry();
    let unavailable = (
        Registration::Unavailable,
        Some("snapshot_worker_unavailable"),
    );
    if registry.disconnected {
        return unavailable;
    }
    if registry.entries.len() >= MAX_TRACKED && !registry.entries.contains_key(&context.key) {
        registry.overflow += 1;
        return (Registration::Overflow, Some("snapshot_control_busy"));
    }
    registry.next_generation += 1;
    let generation = registry.next_generation;
    let full = registry.queued >= QUEUED_BACKUPS;
    let entry = registry.entries.entry(context.key.clone()).or_default();
    entry.generation = generation;
    entry.active += 1;
    entry.restored = false;
    entry.resolution = None;
    let reason = if snapshot.len() as u64 > crate::platform::draft_editor::MAX_BYTES {
        Some("snapshot_too_large")
    } else if full {
        Some("snapshot_worker_busy")
    } else if service
        .work
        .send(Work::Save(context.clone(), snapshot.to_owned()))
        .is_err()
    {
        registry.disconnect();
        return unavailable;
    } else {
        // Enqueue while registering so completion cannot overtake its save.
        entry.saving += 1;
        registry.queued += 1;
        None
    };
    (Registration::Tracked(generation), reason)
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

fn prune(directory: &Path, current: &Path, can_discard: impl Fn(&str) -> bool) -> io::Result<bool> {
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
        if record.path != current
            && discard_if(&record.path, &record.digest, || can_discard(&record.digest))?
        {
            count -= 1;
            bytes = bytes.saturating_sub(record.metadata.len());
        }
    }
    Ok(count <= MAX_RECORDS && bytes <= MAX_BYTES)
}

// Stage the pathname change reversibly. The final authorization after
// rename is the deletion's linearization point. A registration arriving
// during rename wins and the worker puts the original name back. If Bus
// exits during staging, the alternate name is still a managed record.
fn discard_if(path: &Path, digest: &str, allowed: impl Fn() -> bool) -> io::Result<bool> {
    if !allowed() {
        return Ok(false);
    }
    let sequence = crate::utils::time::now_ns();
    let staged = path.with_file_name(format!(
        "copied-{digest}-{}-{sequence}.txt",
        std::process::id()
    ));
    fs::rename(path, &staged)?;
    #[cfg(test)]
    test_hooks::before_effect(path);
    if !allowed() {
        fs::rename(staged, path)?;
        return Ok(false);
    }
    fs::remove_file(staged)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    type Action = Box<dyn FnOnce() -> io::Result<()> + Send>;
    static WRITES: OnceLock<Mutex<HashMap<PathBuf, Action>>> = OnceLock::new();

    type Effect = Box<dyn FnOnce() + Send>;
    static EFFECTS: OnceLock<Mutex<HashMap<PathBuf, Effect>>> = OnceLock::new();

    pub(super) fn install_effect(path: PathBuf, effect: impl FnOnce() + Send + 'static) {
        let mut effects = EFFECTS.get_or_init(Default::default).lock().unwrap();
        assert!(effects.insert(path, Box::new(effect)).is_none());
    }

    pub(super) fn before_effect(path: &Path) {
        let effect = EFFECTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(path);
        if let Some(effect) = effect {
            effect();
        }
    }

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

#[cfg(test)]
#[path = "../../tests/copy_recovery_test.rs"]
mod tests;
