use crate::platform::windows::process::foreground::descendant_entries;
use crate::platform::windows::process::foreground::process_entry_identifies_agent;
use crate::platform::windows::process::peb::command_line_to_argv;
use crate::platform::windows::process::peb::nul_terminated_utf16_to_string;
use crate::platform::windows::process::peb::process_creation_time;
use crate::platform::windows::process::peb::read_process_command;
use crate::platform::windows::process::peb::ProcessHandle;
use crate::platform::windows::process::peb::STILL_ACTIVE;
use std::collections::HashMap;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot;
use windows_sys::Win32::System::Diagnostics::ToolHelp::Process32FirstW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::Process32NextW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W;
use windows_sys::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;

pub(in crate::platform::windows) const FOREGROUND_PROCESS_SNAPSHOT_CACHE_TTL: Duration =
    Duration::from_millis(250);

#[derive(Debug)]
pub(in crate::platform::windows) struct CachedProcessSnapshot {
    pub(in crate::platform::windows) built_at: Instant,
    pub(in crate::platform::windows) snapshot: Arc<ProcessSnapshot>,
}

#[derive(Debug)]
pub(in crate::platform::windows) struct ProcessSnapshotCache {
    pub(in crate::platform::windows) cached: Option<CachedProcessSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::platform::windows) struct ProcessSignature {
    pub(in crate::platform::windows) pid: u32,
    pub(in crate::platform::windows) parent_pid: u32,
    pub(in crate::platform::windows) name: String,
}

impl ProcessSignature {
    pub(in crate::platform::windows) fn from_entry(entry: &WindowsProcessEntry) -> Self {
        Self {
            pid: entry.pid,
            parent_pid: entry.parent_pid,
            name: entry.name.clone(),
        }
    }

    pub(in crate::platform::windows) fn matches(
        &self,
        entry: Option<&WindowsProcessEntry>,
    ) -> bool {
        entry.is_some_and(|entry| {
            self.pid == entry.pid && self.parent_pid == entry.parent_pid && self.name == entry.name
        })
    }
}

#[derive(Debug)]
pub(in crate::platform::windows) enum ProcessIdentity {
    Handle(OwnedHandle),
    #[cfg(test)]
    Stub {
        running: bool,
        creation_time: Option<u64>,
    },
}

impl ProcessIdentity {
    pub(in crate::platform::windows) fn open(pid: u32) -> Option<Self> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
        Some(Self::Handle(handle))
    }

    pub(in crate::platform::windows) fn running(&self) -> bool {
        match self {
            Self::Handle(handle) => {
                let mut exit_code = 0;
                let read = unsafe {
                    GetExitCodeProcess(handle.as_raw_handle().cast(), &mut exit_code) != 0
                };
                read && exit_code == STILL_ACTIVE
            }
            #[cfg(test)]
            Self::Stub { running, .. } => *running,
        }
    }

    pub(in crate::platform::windows) fn creation_time(&self) -> Option<u64> {
        match self {
            Self::Handle(handle) => process_creation_time(handle.as_raw_handle().cast()),
            #[cfg(test)]
            Self::Stub { creation_time, .. } => *creation_time,
        }
    }
}

pub(in crate::platform::windows) static FOREGROUND_PROCESS_SNAPSHOT_CACHE: Mutex<
    ProcessSnapshotCache,
> = Mutex::new(ProcessSnapshotCache { cached: None });

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::platform::windows) struct WindowsProcessCommand {
    pub(in crate::platform::windows) creation_time: Option<u64>,
    pub(in crate::platform::windows) argv0: Option<String>,
    pub(in crate::platform::windows) argv: Option<Vec<String>>,
    pub(in crate::platform::windows) cmdline: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::platform::windows) struct WindowsProcessEntry {
    pub(in crate::platform::windows) pid: u32,
    pub(in crate::platform::windows) parent_pid: u32,
    pub(in crate::platform::windows) name: String,
    pub(in crate::platform::windows) command: OnceLock<WindowsProcessCommand>,
}

impl WindowsProcessEntry {
    pub(in crate::platform::windows) fn command(&self) -> &WindowsProcessCommand {
        self.command
            .get_or_init(|| read_process_command(self.pid, &self.name))
    }
}

#[derive(Debug)]
pub(in crate::platform::windows) struct ProcessSnapshot {
    pub(in crate::platform::windows) entries: Vec<WindowsProcessEntry>,
    pub(in crate::platform::windows) entry_by_pid: HashMap<u32, usize>,
    pub(in crate::platform::windows) children_by_parent: HashMap<u32, Vec<usize>>,
    pub(in crate::platform::windows) agent_indices: OnceLock<Vec<usize>>,
}

impl ProcessSnapshot {
    pub(in crate::platform::windows) fn new(entries: Vec<WindowsProcessEntry>) -> Self {
        let mut entry_by_pid = HashMap::with_capacity(entries.len());
        let mut children_by_parent = HashMap::<u32, Vec<usize>>::new();
        for (index, entry) in entries.iter().enumerate() {
            entry_by_pid.insert(entry.pid, index);
            children_by_parent
                .entry(entry.parent_pid)
                .or_default()
                .push(index);
        }
        Self {
            entries,
            entry_by_pid,
            children_by_parent,
            agent_indices: OnceLock::new(),
        }
    }

    pub(in crate::platform::windows) fn entry(&self, pid: u32) -> Option<&WindowsProcessEntry> {
        self.entry_by_pid
            .get(&pid)
            .map(|&index| &self.entries[index])
    }

    pub(in crate::platform::windows) fn descendant_signatures(
        &self,
        root_pid: u32,
    ) -> Vec<ProcessSignature> {
        let mut signatures = descendant_entries(root_pid, self)
            .into_iter()
            .map(ProcessSignature::from_entry)
            .collect::<Vec<_>>();
        signatures.sort_unstable_by_key(|entry| entry.pid);
        signatures
    }

    pub(in crate::platform::windows) fn agent_indices(&self) -> &[usize] {
        self.agent_indices.get_or_init(|| {
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| process_entry_identifies_agent(entry).then_some(index))
                .collect()
        })
    }
}

pub(in crate::platform::windows) fn snapshot_processes() -> Vec<WindowsProcessEntry> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let _snapshot = ProcessHandle(snapshot);

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut output = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let pid = entry.th32ProcessID;
        let name = nul_terminated_utf16_to_string(&entry.szExeFile);
        output.push(WindowsProcessEntry {
            pid,
            parent_pid: entry.th32ParentProcessID,
            name,
            command: OnceLock::new(),
        });
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    output
}

pub(in crate::platform::windows) fn cached_foreground_processes() -> Arc<ProcessSnapshot> {
    let mut cache = FOREGROUND_PROCESS_SNAPSHOT_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    cache.snapshot(FOREGROUND_PROCESS_SNAPSHOT_CACHE_TTL, snapshot_processes)
}

pub(in crate::platform::windows) fn fresh_foreground_processes() -> Arc<ProcessSnapshot> {
    let mut cache = FOREGROUND_PROCESS_SNAPSHOT_CACHE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    cache.snapshot(Duration::ZERO, snapshot_processes)
}

impl ProcessSnapshotCache {
    pub(in crate::platform::windows) fn snapshot(
        &mut self,
        max_age: Duration,
        build: impl FnOnce() -> Vec<WindowsProcessEntry>,
    ) -> Arc<ProcessSnapshot> {
        if let Some(cached) = &self.cached {
            if cached.built_at.elapsed() < max_age {
                return Arc::clone(&cached.snapshot);
            }
        }

        let snapshot = Arc::new(ProcessSnapshot::new(build()));
        self.cached = Some(CachedProcessSnapshot {
            built_at: Instant::now(),
            snapshot: Arc::clone(&snapshot),
        });
        snapshot
    }
}

impl WindowsProcessCommand {
    pub(in crate::platform::windows) fn from_cmdline(
        name: &str,
        creation_time: Option<u64>,
        cmdline: Option<String>,
    ) -> Self {
        let argv = cmdline.as_deref().and_then(command_line_to_argv);
        let argv0 = argv
            .as_ref()
            .and_then(|argv| argv.first().cloned())
            .or_else(|| (!name.is_empty()).then(|| name.to_string()));
        Self {
            creation_time,
            argv0,
            argv,
            cmdline,
        }
    }
}
