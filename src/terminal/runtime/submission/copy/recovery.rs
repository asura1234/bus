//! Persist copied drafts without holding the pane input lease across storage I/O.
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{mpsc, Mutex, MutexGuard, OnceLock},
    thread,
};

mod caller;
mod storage;
pub(in crate::terminal::runtime::submission) use caller::CallerDiagnostics;
use storage::{discard_if, prune, save, verified_records, PruneDecision, PruneResult};

const QUEUED_BACKUPS: usize = 16;
const MAX_TRACKED: usize = 256;
const MAX_OVERFLOW: usize = 64;
const MAX_DEFERRED: usize = 64;

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
struct Terminal {
    generation: u64,
    active: usize,
    control_queued: bool,
    settling: bool,
    restored: bool,
    resolution: Option<Resolution>,
}

impl Terminal {
    fn begin(&mut self, generation: u64) {
        self.generation = generation;
        self.active += 1;
        self.restored = false;
        self.resolution = None;
    }

    fn protected(&self) -> bool {
        self.active != 0 || self.control_queued || self.settling || self.resolution.is_some()
    }

    fn queue_if_ready(&mut self, key: Key, save_pending: bool) -> Option<Work> {
        if self.active != 0 || save_pending || self.control_queued || self.resolution.is_none() {
            return None;
        }
        self.control_queued = true;
        Some(Work::Settle(key))
    }
}

#[derive(Default)]
struct Entry {
    terminal: Terminal,
    saving: usize,
}

#[derive(Default)]
struct OverflowEntry {
    terminal: Terminal,
    pin_next_prune: bool,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<Key, Entry>,
    overflow: HashMap<Key, OverflowEntry>,
    deferred: VecDeque<(PathBuf, PathBuf)>,
    next_generation: u64,
    queued: usize,
    untracked: usize,
    revisit_queued: bool,
    disconnected: bool,
}

impl Registry {
    fn terminal(&self, key: &Key) -> Option<&Terminal> {
        self.entries
            .get(key)
            .map(|entry| &entry.terminal)
            .or_else(|| self.overflow.get(key).map(|entry| &entry.terminal))
    }

    fn terminal_mut(&mut self, key: &Key) -> Option<&mut Terminal> {
        self.entries
            .get_mut(key)
            .map(|entry| &mut entry.terminal)
            .or_else(|| self.overflow.get_mut(key).map(|entry| &mut entry.terminal))
    }

    fn discard_idle(&mut self, key: &Key) {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.saving == 0 && !entry.terminal.protected())
        {
            self.entries.remove(key);
        }
        if self
            .overflow
            .get(key)
            .is_some_and(|entry| !entry.pin_next_prune && !entry.terminal.protected())
        {
            self.overflow.remove(key);
        }
    }

    fn current(&self, key: &Key, generation: u64) -> bool {
        self.terminal(key)
            .is_some_and(|state| state.active == 0 && state.generation == generation)
    }

    fn prune_decision(&self, key: &Key) -> PruneDecision {
        let overflow = self.overflow.get(key);
        if self.untracked != 0
            || self
                .entries
                .get(key)
                .is_some_and(|entry| entry.saving != 0 || entry.terminal.protected())
            || overflow.is_some_and(|entry| entry.terminal.protected())
        {
            PruneDecision::Keep
        } else if overflow.is_some_and(|entry| entry.pin_next_prune) {
            PruneDecision::HonorPin
        } else {
            PruneDecision::Discard
        }
    }

    fn disconnect(&mut self) -> Option<Abandoned> {
        if self.disconnected {
            return None;
        }
        let terminals = self
            .entries
            .values()
            .map(|entry| &entry.terminal)
            .chain(self.overflow.values().map(|entry| &entry.terminal));
        let mut resolutions = 0;
        let mut active = self.untracked;
        for terminal in terminals {
            resolutions +=
                usize::from(terminal.resolution.is_some()) + usize::from(terminal.settling);
            active += terminal.active;
        }
        let abandoned = Abandoned {
            resolutions,
            active,
            saves: self.entries.values().map(|entry| entry.saving).sum(),
            deferrals: self.deferred.len(),
        };
        *self = Self {
            disconnected: true,
            ..Self::default()
        };
        Some(abandoned)
    }
}

struct Abandoned {
    resolutions: usize,
    active: usize,
    saves: usize,
    deferrals: usize,
}

fn report_disconnect(reason: &'static str, abandoned: Option<Abandoned>) {
    if let Some(Abandoned {
        resolutions,
        active,
        saves,
        deferrals,
    }) = abandoned
    {
        tracing::warn!(
            event = "bus.draft.copy_recovery_disconnected",
            reason,
            resolutions,
            active,
            saves,
            deferrals,
            "Copied draft worker disconnected; durable records remain available for a later scan"
        );
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
        let abandoned = self.0.registry().disconnect();
        report_disconnect("worker_exit", abandoned);
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
            self.step(work);
        }
    }

    fn step(&self, work: Work) {
        match work {
            Work::Save(context, snapshot) => {
                tracing::dispatcher::with_default(&context.dispatch, || {
                    self.persist(&context.key, snapshot);
                });
            }
            Work::Settle(key) => self.settle(&key),
            Work::Revisit => {
                let directories = {
                    let mut registry = self.registry();
                    registry.revisit_queued = false;
                    registry
                        .deferred
                        .iter()
                        .map(|(directory, _)| directory.clone())
                        .collect::<Vec<_>>()
                };
                for directory in directories {
                    self.revisit_retention(&directory, false);
                }
            }
        }
    }

    fn persist(&self, key: &Key, snapshot: String) {
        let should_save = {
            let mut registry = self.registry();
            registry.queued -= 1;
            registry
                .terminal(key)
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
        let abandoned = {
            let mut registry = self.registry();
            let work = registry.entries.get_mut(key).and_then(|entry| {
                entry.saving -= 1;
                entry
                    .terminal
                    .queue_if_ready(key.clone(), entry.saving != 0)
            });
            registry.discard_idle(key);
            self.send_control(&mut registry, work)
        };
        report_disconnect("control_send_failed", abandoned);
        self.revisit_retention(&key.directory, false);
    }

    fn send_control(&self, registry: &mut Registry, work: Option<Work>) -> Option<Abandoned> {
        if work.is_some_and(|work| self.work.send(work).is_err()) {
            registry.disconnect()
        } else {
            None
        }
    }

    fn settle(&self, key: &Key) {
        let resolution = {
            let mut registry = self.registry();
            let save_pending = registry
                .entries
                .get(key)
                .is_some_and(|entry| entry.saving != 0);
            let Some(state) = registry.terminal_mut(key) else {
                return;
            };
            state.control_queued = false;
            // Every admitted save for this digest can supply the latest
            // resolution, even when that attempt could not queue its own save.
            // The last such save will wake the control.
            if state.active == 0 && !save_pending {
                state.settling = state.resolution.is_some();
                state.resolution.take()
            } else {
                None
            }
        };
        let mut check_idle_pin = false;
        if let Some(resolution) = resolution {
            let generation = resolution.generation;
            let pin = tracing::dispatcher::with_default(&resolution.context.dispatch, || {
                let backups = verified_records(key).unwrap_or_else(|error| {
                    tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_lookup_failed", %error, "Could not verify copied draft recovery records");
                    Vec::new()
                });
                self.apply_outcome(key, generation, resolution.outcome, backups)
            });
            let mut registry = self.registry();
            if let Some(entry) = registry.overflow.get_mut(key) {
                if entry.terminal.generation == generation {
                    entry.pin_next_prune = pin;
                    check_idle_pin = pin;
                }
            }
            if let Some(state) = registry.terminal_mut(key) {
                state.settling = false;
            }
            registry.discard_idle(key);
        }
        self.revisit_retention(&key.directory, check_idle_pin);
    }

    fn apply_outcome(
        &self,
        key: &Key,
        generation: u64,
        outcome: Outcome,
        backups: Vec<PathBuf>,
    ) -> bool {
        if let Some(path) = backups.first() {
            if self.registry().current(key, generation) {
                self.enforce_retention(key, path);
            }
        }
        match outcome {
            Outcome::Restored => {
                self.retire(key, generation, backups);
                false
            }
            Outcome::Retain { reason, error } => {
                #[cfg(test)]
                if let Some(path) = backups.first() {
                    test_hooks::before_effect(path);
                }
                let notice = {
                    let registry = self.registry();
                    // Commit the owned event under the generation lock. The
                    // sink can do blocking I/O after this decision is final.
                    registry.current(key, generation).then(|| Notice {
                        path: backups.into_iter().next(),
                        reason,
                        error,
                    })
                };
                notice.is_some_and(Notice::emit)
            }
        }
    }

    fn retire(&self, key: &Key, generation: u64, backups: Vec<PathBuf>) {
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
        let current = self.registry().current(key, generation);
        if current {
            tracing::debug!(
                event = "bus.draft.copy_recovery_retired",
                "Verified restore has no remaining recovery record"
            );
        }
    }

    fn enforce_retention(&self, key: &Key, current: &Path) {
        self.prune_directory(&key.directory, current);
    }

    fn revisit_retention(&self, directory: &Path, check_idle_pin: bool) {
        let current = self
            .registry()
            .deferred
            .iter()
            .find(|(dir, _)| dir == directory)
            .map(|(_, path)| path.clone());
        if let Some(current) = current {
            self.prune_directory(directory, &current);
        } else if check_idle_pin {
            // An idle pane still needs a cap check to release its pin.
            self.prune_directory(directory, Path::new(""));
        }
    }

    fn prune_directory(&self, directory: &Path, current: &Path) {
        let result = prune(directory, current, |digest| {
            let candidate = Key {
                directory: directory.to_owned(),
                digest: digest.to_owned(),
            };
            self.registry().prune_decision(&candidate)
        });
        let (evicted, error) = self.record_prune_result(directory, current, result);
        if evicted {
            tracing::warn!(
                event = "bus.draft.copy_recovery_failed",
                reason = "snapshot_deferral_limit",
                "Dropped oldest deferred scan; durable files remain for a later scan"
            );
        }
        if let Some(error) = error {
            tracing::warn!(event = "bus.draft.copy_recovery_failed", reason = "snapshot_prune_failed", %error, "Could not prune copied draft recovery records");
        }
    }

    fn record_prune_result(
        &self,
        directory: &Path,
        current: &Path,
        result: std::io::Result<PruneResult>,
    ) -> (bool, Option<std::io::Error>) {
        let mut registry = self.registry();
        if let Ok(pass) = &result {
            registry.overflow.retain(|key, entry| {
                if key.directory == directory
                    && (pass.within_cap || pass.honored_pins.contains(&key.digest))
                {
                    entry.pin_next_prune = false;
                }
                entry.pin_next_prune || entry.terminal.protected()
            });
        }
        match result {
            Ok(pass) if pass.within_cap => {
                registry.deferred.retain(|(dir, _)| dir != directory);
                (false, None)
            }
            Ok(_) => {
                if let Some((_, path)) = registry
                    .deferred
                    .iter_mut()
                    .find(|(dir, _)| dir == directory)
                {
                    current.clone_into(path);
                    return (false, None);
                }
                let evicted = registry.deferred.len() == MAX_DEFERRED;
                if evicted {
                    registry.deferred.pop_front();
                }
                registry
                    .deferred
                    .push_back((directory.to_owned(), current.to_owned()));
                (evicted, None)
            }
            Err(error) => {
                if registry.untracked == 0
                    && !registry
                        .entries
                        .keys()
                        .chain(registry.overflow.keys())
                        .any(|key| key.directory == directory)
                {
                    registry.deferred.retain(|(dir, _)| dir != directory);
                }
                (false, Some(error))
            }
        }
    }
}

struct Notice {
    path: Option<PathBuf>,
    reason: &'static str,
    error: String,
}

impl Notice {
    fn emit(self) -> bool {
        let Self {
            path,
            reason,
            error,
        } = self;
        if let Some(path) = path {
            tracing::warn!(event = "bus.draft.recovery_required", reason, recovery = %path.display(), %error, "Copied composer snapshot retained for manual recovery");
            true
        } else {
            tracing::warn!(event = "bus.draft.copy_loss", reason = "snapshot_storage_unavailable", draft_outcome = reason, %error, "Copied draft restoration was not proven and no verified recovery record is available");
            false
        }
    }
}

#[derive(Clone, Copy)]
enum Registration {
    Tracked(u64),
    Overflow(u64),
    Untracked,
    Unavailable,
}

pub(super) struct Attempt<'a> {
    service: &'static Service,
    context: Option<Context>,
    registration: Registration,
    diagnostics: Option<&'a CallerDiagnostics>,
}

impl Attempt<'_> {
    pub(super) fn send(mut self, outcome: Outcome) {
        self.finish(outcome);
    }

    fn finish(&mut self, outcome: Outcome) {
        let Some(context) = self.context.take() else {
            return;
        };
        let untracked = matches!(self.registration, Registration::Untracked);
        let abandoned = {
            let mut registry = self.service.registry();
            if registry.disconnected {
                return;
            }
            let work = match self.registration {
                Registration::Tracked(generation) | Registration::Overflow(generation) => {
                    finish_tracked(&mut registry, context, generation, outcome)
                }
                Registration::Untracked => {
                    registry.untracked -= 1;
                    let wake = registry.untracked == 0 && !registry.revisit_queued;
                    registry.revisit_queued |= wake;
                    wake.then_some(Work::Revisit)
                }
                Registration::Unavailable => None,
            };
            self.service.send_control(&mut registry, work)
        };
        if let Some(diagnostics) = self.diagnostics {
            diagnostics.disconnected("control_send_failed", abandoned);
            if untracked {
                diagnostics.overflow_outcome_dropped();
            }
        }
    }
}

impl Drop for Attempt<'_> {
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
    let save_pending = registry
        .entries
        .get(&key)
        .is_some_and(|entry| entry.saving != 0);
    let terminal = registry.terminal_mut(&key)?;
    terminal.active -= 1;
    if terminal.generation == generation {
        terminal.restored = matches!(outcome, Outcome::Restored);
        terminal.resolution = Some(Resolution {
            context,
            generation,
            outcome,
        });
    }
    terminal.queue_if_ready(key, save_pending)
}

pub(super) fn start<'a>(
    directory: &Path,
    snapshot: &str,
    diagnostics: &'a CallerDiagnostics,
) -> Option<Attempt<'a>> {
    if snapshot.is_empty() {
        return None;
    }
    static WORKER: OnceLock<std::io::Result<Service>> = OnceLock::new();
    let service = WORKER.get_or_init(|| {
        let (work, receive) = mpsc::channel();
        let service = Service {
            work,
            registry: Default::default(),
        };
        let worker = service.clone();
        thread::Builder::new()
            .name("copy-draft-recovery".into())
            .spawn(move || worker.run(receive))
            .map(|_| service)
    });
    let service = match service {
        Ok(service) => service,
        Err(error) => {
            diagnostics.startup_failed(error.to_string());
            return None;
        }
    };
    let context = Context {
        key: Key {
            directory: directory.to_owned(),
            digest: crate::utils::time::digest(snapshot.as_bytes()),
        },
        dispatch: tracing::dispatcher::get_default(Clone::clone),
    };
    let (registration, skipped_reason) = register(service, &context, snapshot, Some(diagnostics));
    if let Some(reason) = skipped_reason {
        diagnostics.admission_skipped(reason);
    }
    if matches!(registration, Registration::Unavailable) {
        return None;
    }
    Some(Attempt {
        service,
        registration,
        context: Some(context),
        diagnostics: Some(diagnostics),
    })
}

fn register(
    service: &Service,
    context: &Context,
    snapshot: &str,
    diagnostics: Option<&CallerDiagnostics>,
) -> (Registration, Option<&'static str>) {
    let mut registry = service.registry();
    let unavailable = (
        Registration::Unavailable,
        Some("snapshot_worker_unavailable"),
    );
    if registry.disconnected {
        return unavailable;
    }
    registry.next_generation += 1;
    let generation = registry.next_generation;
    if !registry.entries.contains_key(&context.key)
        && (registry.entries.len() >= MAX_TRACKED || registry.overflow.contains_key(&context.key))
    {
        if registry.overflow.len() >= MAX_OVERFLOW && !registry.overflow.contains_key(&context.key)
        {
            registry.untracked += 1;
            return (Registration::Untracked, Some("snapshot_control_busy"));
        }
        registry
            .overflow
            .entry(context.key.clone())
            .or_default()
            .terminal
            .begin(generation);
        return (
            Registration::Overflow(generation),
            Some("snapshot_overflow_control_only"),
        );
    }
    let full = registry.queued >= QUEUED_BACKUPS;
    let entry = registry.entries.entry(context.key.clone()).or_default();
    entry.terminal.begin(generation);
    let reason = if snapshot.len() as u64 > crate::platform::draft_editor::MAX_BYTES {
        Some("snapshot_too_large")
    } else if full {
        Some("snapshot_worker_busy")
    } else if service
        .work
        .send(Work::Save(context.clone(), snapshot.to_owned()))
        .is_err()
    {
        let abandoned = registry.disconnect();
        drop(registry);
        if let Some(diagnostics) = diagnostics {
            diagnostics.disconnected("snapshot_send_failed", abandoned);
        }
        return unavailable;
    } else {
        // Enqueue while registering so completion cannot overtake its save.
        entry.saving += 1;
        registry.queued += 1;
        None
    };
    (Registration::Tracked(generation), reason)
}

#[cfg(test)]
#[path = "recovery/test_hooks_test.rs"]
pub(crate) mod test_hooks;

#[cfg(test)]
#[path = "../../tests/copy_recovery_test.rs"]
mod tests;
