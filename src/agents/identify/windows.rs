//! Provider interpretation of Windows process trees; OS reads and lifetimes stay platform.
use crate::platform::{ForegroundJob, ProcessSnapshot, WindowsProcessEntry};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

struct AgentProcessSnapshot {
    raw: Arc<ProcessSnapshot>,
    agent_indices: OnceLock<Vec<usize>>,
}

impl std::ops::Deref for AgentProcessSnapshot {
    type Target = ProcessSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

impl AgentProcessSnapshot {
    fn agent_indices(&self) -> &[usize] {
        self.agent_indices.get_or_init(|| {
            self.raw
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| process_entry_identifies_agent(entry).then_some(index))
                .collect()
        })
    }

    #[cfg(test)]
    fn new(entries: Vec<WindowsProcessEntry>) -> Self {
        Self {
            raw: Arc::new(ProcessSnapshot::new(entries)),
            agent_indices: OnceLock::new(),
        }
    }
}

// Each raw snapshot is shared across pane probes. Keep its provider-candidate
// memoization here, rather than teaching the platform cache about agents.
static AGENT_SNAPSHOT: LazyLock<Mutex<Option<Arc<AgentProcessSnapshot>>>> =
    LazyLock::new(|| Mutex::new(None));

fn interpreted_snapshot(raw: &Arc<ProcessSnapshot>) -> Arc<AgentProcessSnapshot> {
    let mut cached = AGENT_SNAPSHOT.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(snapshot) = cached
        .as_ref()
        .filter(|snapshot| Arc::ptr_eq(&snapshot.raw, raw))
    {
        return Arc::clone(snapshot);
    }
    let snapshot = Arc::new(AgentProcessSnapshot {
        raw: Arc::clone(raw),
        agent_indices: OnceLock::new(),
    });
    *cached = Some(Arc::clone(&snapshot));
    snapshot
}

pub(super) fn foreground_job(shell_pid: u32) -> Option<ForegroundJob> {
    crate::platform::pane_foreground_job_with(shell_pid, |pid, raw| {
        select_pane_foreground_job_from_snapshot_uncached(pid, &interpreted_snapshot(raw))
    })
}

fn select_pane_foreground_job_from_snapshot_uncached(
    shell_pid: u32,
    snapshot: &AgentProcessSnapshot,
) -> Option<ForegroundJob> {
    select_pane_foreground_job_from_snapshot_with_runtime_inspection(
        shell_pid,
        snapshot,
        |shell| shell.is_git_bash(),
        |entry| entry.runtime_marker(),
    )
}

fn select_pane_foreground_job_from_snapshot_with_runtime_inspection(
    shell_pid: u32,
    snapshot: &AgentProcessSnapshot,
    shell_is_git_bash: impl FnOnce(&WindowsProcessEntry) -> bool,
    mut runtime_marker: impl FnMut(&WindowsProcessEntry) -> Option<String>,
) -> Option<ForegroundJob> {
    let entries = &snapshot.entries;
    let shell = snapshot.entry(shell_pid)?;
    let descendants = snapshot.raw.descendants(shell_pid);
    let mut candidates = Vec::new();
    for entry in std::iter::once(shell).chain(descendants) {
        if process_entry_identifies_agent(entry) {
            candidates.push(entry);
        }
    }

    if let Some(selected) = select_topmost_agent_chain_candidate(&candidates, snapshot) {
        return Some(selected.foreground_job());
    }
    if !candidates.is_empty() || !shell_is_git_bash(shell) {
        return Some(shell.foreground_job());
    }

    let escaped_agent_indices = snapshot.agent_indices();
    if escaped_agent_indices.is_empty() {
        return Some(shell.foreground_job());
    }

    let Some(shell_runtime_marker) = runtime_marker(shell).filter(|marker| !marker.is_empty())
    else {
        return Some(shell.foreground_job());
    };
    let matching_candidates: Vec<_> = escaped_agent_indices
        .iter()
        .map(|&index| &entries[index])
        .filter(|entry| runtime_marker(entry).as_deref() == Some(shell_runtime_marker.as_str()))
        .collect();
    let selected =
        select_topmost_agent_chain_candidate(&matching_candidates, snapshot).unwrap_or(shell);
    Some(selected.foreground_job())
}

#[cfg(test)]
fn select_pane_foreground_job(
    shell_pid: u32,
    entries: &[WindowsProcessEntry],
) -> Option<ForegroundJob> {
    select_pane_foreground_job_from_snapshot_uncached(
        shell_pid,
        &AgentProcessSnapshot::new(entries.to_vec()),
    )
}

fn process_entry_identifies_agent(entry: &WindowsProcessEntry) -> bool {
    super::identify_agent(&entry.name).is_some()
        || super::identify_agent_in_job(&entry.foreground_job()).is_some()
}

fn select_topmost_agent_chain_candidate<'a>(
    candidates: &[&'a WindowsProcessEntry],
    snapshot: &AgentProcessSnapshot,
) -> Option<&'a WindowsProcessEntry> {
    if candidates.is_empty() {
        return None;
    }

    candidates.iter().copied().find(|entry| {
        candidates.iter().all(|other| {
            entry.pid == other.pid || process_is_ancestor(entry.pid, other.pid, snapshot)
        })
    })
}

fn process_is_ancestor(
    ancestor_pid: u32,
    descendant_pid: u32,
    snapshot: &AgentProcessSnapshot,
) -> bool {
    let mut current = descendant_pid;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        let Some(parent) = snapshot.entry(current).map(|entry| entry.parent_pid) else {
            return false;
        };
        if parent == ancestor_pid {
            return true;
        }
        if parent == 0 {
            return false;
        }
        current = parent;
    }

    false
}

#[cfg(test)]
#[path = "windows/selection_test.rs"]
mod tests;
