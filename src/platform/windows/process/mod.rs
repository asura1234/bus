//! Process control and pane process facts.
pub(super) mod foreground;
pub(super) mod peb;
pub(super) mod snapshot;
use crate::platform::windows::process::foreground::descendant_entries;
use crate::platform::windows::process::foreground::foreground_process_from_entry;
use crate::platform::windows::process::foreground::select_pane_foreground_job_cached;
use crate::platform::windows::process::peb::read_process_parameters;
use crate::platform::windows::process::peb::read_unicode_string;
use crate::platform::windows::process::peb::ProcessHandle;
use crate::platform::windows::process::peb::STILL_ACTIVE;
use crate::platform::windows::process::snapshot::cached_foreground_processes;
use crate::platform::windows::process::snapshot::snapshot_processes;
use crate::platform::windows::process::snapshot::ProcessSnapshot;
use crate::platform::ForegroundJob;
use crate::platform::Signal;
use std::path::PathBuf;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::TerminateProcess;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows_sys::Win32::System::Threading::PROCESS_VM_READ;

pub fn foreground_job(child_pid: u32) -> Option<ForegroundJob> {
    select_pane_foreground_job_cached(child_pid)
}

pub(crate) fn available_pane_shell(child_pid: u32) -> Option<String> {
    let snapshot = ProcessSnapshot::new(snapshot_processes());
    available_pane_shell_from_snapshot(child_pid, &snapshot)
}

pub(in crate::platform::windows) fn available_pane_shell_from_snapshot(
    child_pid: u32,
    snapshot: &ProcessSnapshot,
) -> Option<String> {
    let shell = snapshot.entry(child_pid)?;
    if !crate::platform::is_pane_shell_process_name(&shell.name) {
        return None;
    }
    descendant_entries(child_pid, snapshot)
        .is_empty()
        .then(|| shell.name.clone())
}

pub fn foreground_group_leader_job(process_group_id: u32) -> Option<ForegroundJob> {
    let snapshot = cached_foreground_processes();
    let entry = snapshot.entry(process_group_id)?;
    Some(ForegroundJob {
        process_group_id,
        processes: vec![foreground_process_from_entry(entry)],
    })
}

pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    select_pane_foreground_job_cached(child_pid).map(|job| job.process_group_id)
}

pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    let process = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)?;
    let process_parameters = read_process_parameters(process.0)?;
    read_unicode_string(process.0, process_parameters.current_directory.dos_path)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

pub fn session_processes(child_pid: u32) -> Vec<u32> {
    if child_pid == 0 {
        return Vec::new();
    }

    let snapshot = ProcessSnapshot::new(snapshot_processes());
    session_processes_from_snapshot(child_pid, &snapshot)
}

pub(in crate::platform::windows) fn session_processes_from_snapshot(
    child_pid: u32,
    snapshot: &ProcessSnapshot,
) -> Vec<u32> {
    if snapshot.entry(child_pid).is_none() {
        return Vec::new();
    }

    let mut pids = vec![child_pid];
    pids.extend(
        descendant_entries(child_pid, snapshot)
            .into_iter()
            .map(|entry| entry.pid),
    );
    pids
}

pub fn signal_processes(pids: &[u32], signal: Signal) {
    if signal == Signal::Hangup {
        return;
    }

    for &pid in pids {
        let Some(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            continue;
        };
        unsafe {
            TerminateProcess(process.0, 1);
        }
    }
}

pub fn process_exists(pid: u32) -> bool {
    let Some(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };

    let mut exit_code = 0;
    let ok = unsafe { GetExitCodeProcess(process.0, &mut exit_code) } != 0;
    ok && exit_code == STILL_ACTIVE
}
