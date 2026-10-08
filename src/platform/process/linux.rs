//! Process facts and process control for linux.
#[cfg(test)]
use crate::platform::desktop::linux::bytes_match_image_signature;
#[cfg(test)]
use crate::platform::desktop::linux::clipboard_commands;
#[cfg(test)]
use crate::platform::desktop::linux::read_clipboard_image;
#[cfg(test)]
use crate::platform::desktop::linux::read_clipboard_image_with_spawned_command_max;
#[cfg(test)]
use crate::platform::desktop::linux::read_validated_clipboard_image;
#[cfg(test)]
use crate::platform::desktop::linux::read_wsl_clipboard_image_with_command;
#[cfg(test)]
use crate::platform::desktop::linux::run_clipboard_command;
#[cfg(test)]
use crate::platform::desktop::linux::scrollback_editor_argv;
#[cfg(test)]
use crate::platform::desktop::linux::show_desktop_notification_with_command;
#[cfg(test)]
use crate::platform::desktop::linux::text_indicates_wsl;
#[cfg(test)]
use crate::platform::desktop::linux::write_clipboard;
#[cfg(test)]
use crate::platform::ClipboardCommand;
#[cfg(test)]
use crate::platform::ClipboardImage;
use crate::platform::ForegroundJob;
use crate::platform::ForegroundProcess;
use crate::platform::Signal;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::os::fd::RawFd;
use std::path::PathBuf;
#[cfg(test)]
use std::process::Command;
use std::sync::OnceLock;

#[cfg(test)]
pub(crate) use crate::platform::unix::local_datetime;

pub(crate) use crate::platform::unix::{
    create_remote_private_dir, create_remote_ssh_config_file, hostname, local_datetime_at,
    remote_reattach_program, wait_client_stream_readable,
};

const PROCESS_DETECTION_ENV_VAR: &str = "HERDR_PROCESS_DETECTION";

const CHILD_GROUPS_SCAN_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessDetectionMode {
    Native,
    ChildGroups,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcGroupMember {
    pid: u32,
    comm: String,
}

pub fn raise_server_nofile_limit() {}

fn parse_process_detection_mode(value: Option<&str>) -> Result<ProcessDetectionMode, &str> {
    match value {
        None | Some("") | Some("native") => Ok(ProcessDetectionMode::Native),
        Some("child-groups") => Ok(ProcessDetectionMode::ChildGroups),
        Some(value) => Err(value),
    }
}

fn process_detection_mode() -> ProcessDetectionMode {
    static MODE: OnceLock<ProcessDetectionMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let value = std::env::var(PROCESS_DETECTION_ENV_VAR).ok();
        parse_process_detection_mode(value.as_deref()).unwrap_or_else(|value| {
            tracing::warn!(
                variable = PROCESS_DETECTION_ENV_VAR,
                %value,
                "unknown process detection mode; using native detection"
            );
            ProcessDetectionMode::Native
        })
    })
}

/// Collect the foreground terminal job for a given child PID.
pub(crate) fn available_pane_shell(child_pid: u32) -> Option<String> {
    crate::platform::available_pane_shell_from_job(child_pid, foreground_job(child_pid)?)
}

pub fn foreground_job(child_pid: u32) -> Option<ForegroundJob> {
    if let Some(tpgid) = foreground_process_group_id(child_pid) {
        return foreground_job_for_group(child_pid, tpgid);
    }

    if process_detection_mode() != ProcessDetectionMode::ChildGroups {
        return None;
    }

    foreground_job_for_group(child_pid, child_groups_foreground_process_group(child_pid)?)
}

fn foreground_job_for_group(child_pid: u32, process_group_id: u32) -> Option<ForegroundJob> {
    let members = foreground_process_group_members(child_pid, process_group_id)?;
    let processes = members
        .into_iter()
        .map(|member| {
            let argv = process_argv(member.pid);
            ForegroundProcess {
                pid: member.pid,
                name: member.comm,
                argv0: None,
                cmdline: argv.as_ref().map(|parts| parts.join(" ")),
                argv,
            }
        })
        .collect::<Vec<_>>();

    Some(ForegroundJob {
        process_group_id,
        processes,
    })
}

/// Best-effort foreground group for environments that do not expose terminal
/// foreground groups. This mode is explicit because background jobs cannot be
/// distinguished from foreground jobs without the native terminal signal.
fn child_groups_foreground_process_group(child_pid: u32) -> Option<u32> {
    let shell_group_id = process_pgrp_and_comm(child_pid)
        .map(|(pgrp, _)| pgrp)
        .filter(|pgrp| *pgrp > 0)? as u32;

    child_groups_foreground_process_group_with(
        child_pid,
        shell_group_id,
        process_task_ids,
        process_task_children,
        |pid| process_pgrp_and_comm(pid).map(|(pgrp, _)| pgrp),
    )
}

fn child_groups_foreground_process_group_with(
    child_pid: u32,
    shell_group_id: u32,
    mut task_ids: impl FnMut(u32) -> Vec<u32>,
    mut task_children: impl FnMut(u32, u32) -> Vec<u32>,
    mut process_group_id: impl FnMut(u32) -> Option<i32>,
) -> Option<u32> {
    let mut newest = None;
    let mut scanned = 0usize;
    for tid in task_ids(child_pid) {
        for child in task_children(child_pid, tid) {
            if scanned >= CHILD_GROUPS_SCAN_LIMIT {
                return None;
            }
            scanned += 1;

            let Some(pgrp) = process_group_id(child) else {
                continue;
            };
            if pgrp <= 0 {
                continue;
            }
            let pgrp = pgrp as u32;
            if pgrp == shell_group_id {
                continue;
            }
            newest = Some(newest.map_or(pgrp, |current: u32| current.max(pgrp)));
        }
    }
    newest.or(Some(shell_group_id))
}

fn foreground_process_group_members(
    child_pid: u32,
    process_group_id: u32,
) -> Option<Vec<ProcGroupMember>> {
    foreground_process_group_members_with(
        child_pid,
        process_group_id,
        process_task_ids,
        process_task_children,
        live_process_group_member,
    )
}

fn foreground_process_group_members_with(
    child_pid: u32,
    process_group_id: u32,
    task_ids: impl FnMut(u32) -> Vec<u32>,
    task_children: impl FnMut(u32, u32) -> Vec<u32>,
    mut live_member: impl FnMut(u32, u32) -> Option<ProcGroupMember>,
) -> Option<Vec<ProcGroupMember>> {
    let mut members = process_tree_pids([child_pid, process_group_id], task_ids, task_children)
        .into_iter()
        .filter_map(|pid| live_member(process_group_id, pid))
        .collect::<Vec<_>>();
    members.sort_unstable_by_key(|member| member.pid);
    (!members.is_empty()).then_some(members)
}

fn process_tree_pids(
    roots: impl IntoIterator<Item = u32>,
    mut task_ids: impl FnMut(u32) -> Vec<u32>,
    mut task_children: impl FnMut(u32, u32) -> Vec<u32>,
) -> Vec<u32> {
    let mut pending = VecDeque::new();
    let mut visited = HashSet::new();
    for pid in roots {
        if pid > 0 && visited.insert(pid) {
            pending.push_back(pid);
        }
    }

    let mut pids = Vec::new();
    while let Some(pid) = pending.pop_front() {
        pids.push(pid);
        for tid in task_ids(pid) {
            for child_pid in task_children(pid, tid) {
                if child_pid > 0 && visited.insert(child_pid) {
                    pending.push_back(child_pid);
                }
            }
        }
    }
    pids
}

fn process_task_ids(pid: u32) -> Vec<u32> {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| numeric_file_name(&entry))
        .collect()
}

fn process_task_children(pid: u32, tid: u32) -> Vec<u32> {
    let Some(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/children")).ok()
    else {
        return Vec::new();
    };
    children
        .split_whitespace()
        .filter_map(|child| child.parse::<u32>().ok())
        .collect()
}

fn numeric_file_name(entry: &std::fs::DirEntry) -> Option<u32> {
    let file_name = entry.file_name();
    let value = file_name.to_str()?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn live_process_group_member(process_group_id: u32, pid: u32) -> Option<ProcGroupMember> {
    let (pgrp, comm) = process_pgrp_and_comm(pid)?;
    (pgrp > 0 && pgrp as u32 == process_group_id).then_some(ProcGroupMember { pid, comm })
}

pub fn foreground_group_leader_job(process_group_id: u32) -> Option<ForegroundJob> {
    let (pgrp, name) = process_pgrp_and_comm(process_group_id)?;
    if pgrp as u32 != process_group_id {
        return None;
    }

    let argv = process_argv(process_group_id);
    Some(ForegroundJob {
        process_group_id,
        processes: vec![ForegroundProcess {
            pid: process_group_id,
            name,
            argv0: None,
            cmdline: argv.as_ref().map(|parts| parts.join(" ")),
            argv,
        }],
    })
}

pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    // /proc/<pid>/stat format: "pid (comm) state ppid pgrp session tty_nr tpgid ..."
    // The (comm) field can contain spaces and parens, so we find the last ')' first.
    let stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat")).ok()?;
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After (comm): state(0) ppid(1) pgrp(2) session(3) tty_nr(4) tpgid(5)
    let tpgid: i32 = fields.get(5)?.parse().ok()?;
    (tpgid > 0).then_some(tpgid as u32)
}

pub fn foreground_process_group_id_for_tty_fd(fd: RawFd) -> Option<u32> {
    let pgid = unsafe { libc::tcgetpgrp(fd) };
    (pgid > 0).then_some(pgid as u32)
}

fn process_pgrp_and_comm(pid: u32) -> Option<(i32, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    process_pgrp_and_comm_from_stat(&stat)
}

fn process_pgrp_and_comm_from_stat(stat: &str) -> Option<(i32, String)> {
    let close = stat.rfind(')')?;
    let comm = stat.get(1 + stat.find('(')?..close)?.to_string();
    let rest = stat.get(close + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let pgrp: i32 = fields.get(2)?.parse().ok()?;
    Some((pgrp, comm))
}

fn process_argv(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let parts: Vec<String> = bytes
        .split(|&b| b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    (!parts.is_empty()).then_some(parts)
}

/// Get the current working directory of a process.
/// Uses /proc/<pid>/cwd symlink.
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Read a Herdr agent identity hint from a process environment.
pub fn process_agent_hint(pid: u32) -> Option<crate::detect::Agent> {
    if pid == 0 {
        return None;
    }
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    crate::platform::parse_agent_env_hint(&environ)
}

pub fn session_processes(child_pid: u32) -> Vec<u32> {
    let Some(session_id) = process_session_id(child_pid) else {
        return Vec::new();
    };

    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else {
            continue;
        };
        if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }

        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if process_session_id(pid) == Some(session_id) {
            pids.push(pid);
        }
    }
    pids
}

pub fn signal_processes(pids: &[u32], signal: Signal) {
    let sig = match signal {
        Signal::Hangup => libc::SIGHUP,
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };

    for &pid in pids {
        if pid == 0 {
            continue;
        }
        unsafe {
            libc::kill(pid as i32, sig);
        }
    }
}

pub fn process_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

fn process_session_id(pid: u32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    fields.get(3)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Mutex, OnceLock};

    use std::{cell::RefCell, collections::HashMap};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    include!("../tests/process_linux_test.rs");
    include!("../tests/desktop_linux_test.rs");
}
