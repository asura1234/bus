use crate::platform::windows::process::snapshot::cached_foreground_processes;
use crate::platform::windows::process::snapshot::fresh_foreground_processes;
use crate::platform::windows::process::snapshot::ProcessIdentity;
use crate::platform::windows::process::snapshot::ProcessSignature;
use crate::platform::windows::process::snapshot::ProcessSnapshot;
use crate::platform::windows::process::snapshot::WindowsProcessEntry;
use crate::platform::ForegroundJob;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

pub(in crate::platform::windows) const FOREGROUND_SELECTION_RECHECK: Duration =
    Duration::from_secs(5);

pub(in crate::platform::windows) const FOREGROUND_SELECTION_CACHE_CAPACITY: usize = 1_024;

pub(in crate::platform::windows) const FOREGROUND_SELECTION_CACHE_RETENTION: Duration =
    Duration::from_secs(60);

#[derive(Debug)]
pub(in crate::platform::windows) struct CachedForegroundSelection {
    pub(in crate::platform::windows) shell: ProcessSignature,
    pub(in crate::platform::windows) selected: ProcessSignature,
    pub(in crate::platform::windows) descendants: Vec<ProcessSignature>,
    pub(in crate::platform::windows) descendant_identities: Vec<ProcessIdentity>,
    pub(in crate::platform::windows) shell_identity: ProcessIdentity,
    pub(in crate::platform::windows) selected_identity: ProcessIdentity,
    pub(in crate::platform::windows) job: ForegroundJob,
    pub(in crate::platform::windows) verified_at: Instant,
    pub(in crate::platform::windows) last_used: Instant,
}

#[derive(Debug, Default)]
pub(in crate::platform::windows) struct ForegroundSelectionCache {
    pub(in crate::platform::windows) entries: HashMap<u32, CachedForegroundSelection>,
}

pub(in crate::platform::windows) static FOREGROUND_SELECTION_CACHE: LazyLock<
    Mutex<ForegroundSelectionCache>,
> = LazyLock::new(|| Mutex::new(ForegroundSelectionCache::default()));

pub(crate) fn pane_foreground_job_with(
    shell_pid: u32,
    select: impl Fn(u32, &Arc<ProcessSnapshot>) -> Option<ForegroundJob>,
) -> Option<ForegroundJob> {
    let snapshot = cached_foreground_processes();
    let (job, retry_with_fresh_snapshot) =
        select_pane_foreground_job_from_snapshot(shell_pid, &snapshot, &select)?;
    if !retry_with_fresh_snapshot {
        return Some(job);
    }

    let snapshot = fresh_foreground_processes();
    select_pane_foreground_job_from_snapshot(shell_pid, &snapshot, &select).map(|(job, _)| job)
}

pub(in crate::platform::windows) fn select_pane_foreground_job_from_snapshot(
    shell_pid: u32,
    snapshot: &Arc<ProcessSnapshot>,
    select: &impl Fn(u32, &Arc<ProcessSnapshot>) -> Option<ForegroundJob>,
) -> Option<(ForegroundJob, bool)> {
    if let Some(job) = FOREGROUND_SELECTION_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(shell_pid, snapshot)
    {
        return Some((job, false));
    }

    let job = select(shell_pid, snapshot)?;
    let cached = prepare_cached_foreground_selection(shell_pid, snapshot, &job);
    let retry_with_fresh_snapshot = job.process_group_id != shell_pid && cached.is_none();
    FOREGROUND_SELECTION_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .remember(shell_pid, cached);
    Some((job, retry_with_fresh_snapshot))
}

pub(in crate::platform::windows) fn foreground_job_from_entry(
    entry: &WindowsProcessEntry,
) -> ForegroundJob {
    ForegroundJob {
        process_group_id: entry.pid,
        processes: vec![foreground_process_from_entry(entry)],
    }
}

pub(in crate::platform::windows) fn descendant_entries(
    root_pid: u32,
    snapshot: &ProcessSnapshot,
) -> Vec<&WindowsProcessEntry> {
    let mut output = Vec::new();
    let root_creation_time = snapshot
        .entry(root_pid)
        .and_then(|entry| entry.command().creation_time);
    let mut queue = VecDeque::from([(root_pid, root_creation_time)]);
    let mut visited = HashSet::new();
    visited.insert(root_pid);
    while let Some((parent_pid, parent_creation_time)) = queue.pop_front() {
        let Some(children) = snapshot.children_by_parent.get(&parent_pid) else {
            continue;
        };
        for &index in children {
            let child = &snapshot.entries[index];
            let child_creation_time = child.command().creation_time;
            // ToolHelp retains the original parent PID after its exit. An older child
            // belongs to a previous owner of the parent's reused PID, not this tree.
            if child_creation_time
                .zip(parent_creation_time)
                .is_some_and(|(child, parent)| child < parent)
            {
                continue;
            }
            if visited.insert(child.pid) {
                output.push(child);
                queue.push_back((child.pid, child_creation_time));
            }
        }
    }
    output
}

pub(in crate::platform::windows) fn foreground_process_from_entry(
    entry: &WindowsProcessEntry,
) -> crate::platform::ForegroundProcess {
    let command = entry.command();
    crate::platform::ForegroundProcess {
        pid: entry.pid,
        name: entry.name.clone(),
        argv0: command.argv0.clone(),
        argv: command.argv.clone(),
        cmdline: command.cmdline.clone(),
    }
}

pub(in crate::platform::windows) fn prepare_cached_foreground_selection(
    shell_pid: u32,
    snapshot: &ProcessSnapshot,
    job: &ForegroundJob,
) -> Option<CachedForegroundSelection> {
    if job.process_group_id == shell_pid {
        return None;
    }
    let shell_identity = ProcessIdentity::open(shell_pid)?;
    let selected_identity = ProcessIdentity::open(job.process_group_id)?;
    let descendants = snapshot.descendant_signatures(shell_pid);
    let descendant_identities = descendants
        .iter()
        .map(|entry| ProcessIdentity::open(entry.pid))
        .collect::<Option<Vec<_>>>()?;
    CachedForegroundSelection::from_snapshot_with_identities(
        shell_pid,
        snapshot,
        job,
        descendants,
        descendant_identities,
        shell_identity,
        selected_identity,
    )
}

impl CachedForegroundSelection {
    pub(in crate::platform::windows) fn from_snapshot_with_identities(
        shell_pid: u32,
        snapshot: &ProcessSnapshot,
        job: &ForegroundJob,
        descendants: Vec<ProcessSignature>,
        descendant_identities: Vec<ProcessIdentity>,
        shell_identity: ProcessIdentity,
        selected_identity: ProcessIdentity,
    ) -> Option<Self> {
        if job.process_group_id == shell_pid {
            return None;
        }
        let shell_entry = snapshot.entry(shell_pid)?;
        if shell_identity.creation_time() != shell_entry.command().creation_time {
            return None;
        }
        let shell = ProcessSignature::from_entry(shell_entry);
        let selected_entry = snapshot.entry(job.process_group_id)?;
        if selected_identity.creation_time() != selected_entry.command().creation_time {
            return None;
        }
        let selected = ProcessSignature::from_entry(selected_entry);
        let descendants_match_identities = descendants.len() == descendant_identities.len()
            && descendants
                .iter()
                .zip(&descendant_identities)
                .all(|(signature, identity)| {
                    snapshot.entry(signature.pid).is_some_and(|entry| {
                        identity.creation_time() == entry.command().creation_time
                    })
                });
        if !descendants_match_identities
            || !shell_identity.running()
            || !selected_identity.running()
            || !descendant_identities.iter().all(ProcessIdentity::running)
        {
            return None;
        }
        let now = Instant::now();
        Some(Self {
            shell,
            selected,
            descendants,
            descendant_identities,
            shell_identity,
            selected_identity,
            job: job.clone(),
            verified_at: now,
            last_used: now,
        })
    }
}

impl ForegroundSelectionCache {
    pub(in crate::platform::windows) fn get(
        &mut self,
        shell_pid: u32,
        snapshot: &ProcessSnapshot,
    ) -> Option<ForegroundJob> {
        if let Some(cached) = self.entries.get_mut(&shell_pid) {
            let current_descendants = descendant_entries(shell_pid, snapshot);
            let topology_matches = current_descendants.len() == cached.descendants.len()
                && cached
                    .descendants
                    .iter()
                    .all(|entry| entry.matches(snapshot.entry(entry.pid)));
            let valid = cached.verified_at.elapsed() < FOREGROUND_SELECTION_RECHECK
                && cached.shell_identity.running()
                && cached.selected_identity.running()
                && cached
                    .descendant_identities
                    .iter()
                    .all(ProcessIdentity::running)
                && cached.shell.matches(snapshot.entry(shell_pid))
                && cached
                    .selected
                    .matches(snapshot.entry(cached.job.process_group_id))
                && topology_matches;
            if valid {
                cached.last_used = Instant::now();
                return Some(cached.job.clone());
            }
        }
        self.entries.remove(&shell_pid);
        None
    }

    pub(in crate::platform::windows) fn remember(
        &mut self,
        shell_pid: u32,
        cached: Option<CachedForegroundSelection>,
    ) {
        let Some(cached) = cached else {
            self.entries.remove(&shell_pid);
            return;
        };
        self.entries
            .retain(|_, cached| cached.last_used.elapsed() < FOREGROUND_SELECTION_CACHE_RETENTION);
        if self.entries.len() >= FOREGROUND_SELECTION_CACHE_CAPACITY {
            self.entries.clear();
        }
        self.entries.insert(shell_pid, cached);
    }

    #[cfg(test)]
    pub(in crate::platform::windows) fn remember_for_test(
        &mut self,
        shell_pid: u32,
        snapshot: &ProcessSnapshot,
        job: &ForegroundJob,
    ) {
        let descendants = snapshot.descendant_signatures(shell_pid);
        let descendant_identities = descendants
            .iter()
            .map(|_| ProcessIdentity::Stub {
                running: true,
                creation_time: None,
            })
            .collect();
        let cached = CachedForegroundSelection::from_snapshot_with_identities(
            shell_pid,
            snapshot,
            job,
            descendants,
            descendant_identities,
            ProcessIdentity::Stub {
                running: true,
                creation_time: None,
            },
            ProcessIdentity::Stub {
                running: true,
                creation_time: None,
            },
        );
        self.remember(shell_pid, cached);
    }
}
