//! This process's resource usage and stack captures, for the perf recorder.
//!
//! These return raw facts; the recorder turns deltas into percentages.
use std::path::Path;
use std::time::Duration;

#[cfg(target_os = "macos")]
use crate::utils::perf::ThreadUsage;
use crate::utils::perf::{PerfHooks, ProcessUsage};

/// The recorder's view of this platform.
pub(crate) fn perf_hooks() -> PerfHooks {
    PerfHooks {
        sample: self_usage,
        machine: super::machine_usage::sample,
        local_hm,
        capture_stacks,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn self_usage() -> Option<ProcessUsage> {
    let (rss_bytes, threads) = memory_and_threads()?;
    Some(ProcessUsage {
        cpu_time: self_cpu_time()?,
        rss_bytes,
        threads,
        open_fds: open_fds()?,
        load_1m: load_1m(),
        cpus: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
        system_cpu_ticks: system_cpu_ticks(),
        swap_used_bytes: swap_used_bytes(),
        thread_cpu: thread_cpu(threads),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn self_usage() -> Option<ProcessUsage> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn self_cpu_time() -> Option<Duration> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let time = |tv: libc::timeval| {
        Duration::from_secs(tv.tv_sec.max(0) as u64)
            + Duration::from_micros(tv.tv_usec.max(0) as u64)
    };
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn load_1m() -> Option<f64> {
    let mut load = [0.0_f64; 1];
    (unsafe { libc::getloadavg(load.as_mut_ptr(), 1) } == 1).then_some(load[0])
}

#[cfg(target_os = "macos")]
fn memory_and_threads() -> Option<(u64, u32)> {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    (read == size).then(|| (info.pti_resident_size, info.pti_threadnum.max(0) as u32))
}

/// One bounded listing plus one query per thread, once per perf window. These
/// public libproc IDs are opaque pthread addresses, not Mach port rights.
#[cfg(target_os = "macos")]
fn thread_cpu(threads: u32) -> Option<Vec<ThreadUsage>> {
    const PROC_PIDLISTTHREADS: libc::c_int = 6;
    let pid = std::process::id() as libc::c_int;
    let mut ids = vec![0_u64; (threads as usize + 16).min(4096)];
    let size = (ids.len() * std::mem::size_of::<u64>()) as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid, PROC_PIDLISTTHREADS, 0, ids.as_mut_ptr().cast(), size) };
    if read <= 0 || read >= size || !(read as usize).is_multiple_of(std::mem::size_of::<u64>()) {
        return None;
    }
    ids.truncate(read as usize / std::mem::size_of::<u64>());
    let mut samples = Vec::with_capacity(ids.len());
    for id in ids {
        let mut info: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_threadinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTHREADINFO,
                id,
                (&mut info as *mut libc::proc_threadinfo).cast(),
                size,
            )
        };
        if read != size {
            continue; // Thread exited between the listing and the query.
        }
        let name: Vec<u8> = info
            .pth_name
            .iter()
            .take_while(|ch| **ch != 0)
            .map(|ch| *ch as u8)
            .collect();
        samples.push(ThreadUsage {
            id,
            name: String::from_utf8_lossy(&name).into_owned(),
            // XNU fill_taskthreadinfo exports seconds/microseconds as ns.
            cpu_time: Duration::from_nanos(info.pth_user_time.saturating_add(info.pth_system_time)),
        });
    }
    Some(samples)
}

#[cfg(target_os = "linux")]
fn thread_cpu(_threads: u32) -> Option<Vec<crate::utils::perf::ThreadUsage>> {
    None
}

#[cfg(target_os = "macos")]
fn open_fds() -> Option<u32> {
    let pid = std::process::id() as libc::c_int;
    // With no buffer the call returns the table's capacity in bytes, not the
    // open count; only a listing says how many descriptors are open.
    let capacity =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if capacity <= 0 {
        return None;
    }
    let entry = std::mem::size_of::<libc::proc_fdinfo>();
    let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(capacity as usize / entry + 1);
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            (fds.capacity() * entry) as libc::c_int,
        )
    };
    (written > 0).then(|| written as u32 / entry as u32)
}

#[cfg(target_os = "macos")]
fn system_cpu_ticks() -> Option<(u64, u64)> {
    let mut info: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    unsafe extern "C" {
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    let result = unsafe {
        // Each call returns a send right of its own; release it, or the sampler
        // adds one reference to the host port every second.
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        let result = libc::host_statistics(
            host,
            libc::HOST_CPU_LOAD_INFO,
            &mut info as *mut _ as libc::host_info_t,
            &mut count,
        );
        #[allow(deprecated)]
        mach_port_deallocate(libc::mach_task_self(), host);
        result
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    let ticks = info.cpu_ticks.map(u64::from);
    let total: u64 = ticks.iter().sum();
    Some((total - ticks[libc::CPU_STATE_IDLE as usize], total))
}

#[cfg(target_os = "macos")]
fn swap_used_bytes() -> Option<u64> {
    let mut usage: libc::xsw_usage = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::xsw_usage>();
    let result = unsafe {
        libc::sysctlbyname(
            c"vm.swapusage".as_ptr(),
            &mut usage as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0).then_some(usage.xsu_used)
}

#[cfg(target_os = "linux")]
fn memory_and_threads() -> Option<(u64, u32)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
    };
    Some((field("VmRSS:")? * 1024, field("Threads:")? as u32))
}

#[cfg(target_os = "linux")]
fn open_fds() -> Option<u32> {
    Some(std::fs::read_dir("/proc/self/fd").ok()?.count() as u32)
}

#[cfg(target_os = "linux")]
fn system_cpu_ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let ticks: Vec<u64> = stat
        .lines()
        .next()?
        .strip_prefix("cpu ")?
        .split_whitespace()
        .filter_map(|value| value.parse().ok())
        .collect();
    let total: u64 = ticks.iter().sum();
    // Fields 4 and 5 are idle and iowait.
    let idle = ticks.get(3).copied().unwrap_or(0) + ticks.get(4).copied().unwrap_or(0);
    Some((total.saturating_sub(idle), total))
}

#[cfg(target_os = "linux")]
fn swap_used_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        meminfo
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
    };
    Some(field("SwapTotal:")?.saturating_sub(field("SwapFree:")?) * 1024)
}

/// Samples the stacks of `pid` into `out`, blocking until `sample` exits
/// (about 3 s plus symbolication); true only when it wrote stacks. macOS only.
#[cfg(target_os = "macos")]
pub(crate) fn capture_stacks(pid: u32, out: &Path) -> bool {
    let status = stack_capture_command(pid, out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    status.is_ok_and(|status| status.success())
        && std::fs::metadata(out).is_ok_and(|meta| meta.len() > 0)
}

#[cfg(target_os = "macos")]
fn stack_capture_command(pid: u32, out: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/sample");
    command
        .arg(pid.to_string())
        .arg("3")
        .arg("-mayDie")
        .arg("-file")
        .arg(out);
    command
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn capture_stacks(_pid: u32, _out: &Path) -> bool {
    false
}

#[cfg(test)]
#[path = "tests/usage_test.rs"]
mod usage_test;

/// Only the recorder calls this, with the stall's start wall time.
pub(crate) fn local_hm(seconds: i64) -> (u8, u8) {
    #[cfg(unix)]
    {
        let seconds = seconds as libc::time_t;
        let mut local: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&seconds, &mut local) }.is_null() {
            return (local.tm_hour as u8, local.tm_min as u8);
        }
    }
    let utc = time::OffsetDateTime::from_unix_timestamp(seconds)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    (utc.hour(), utc.minute())
}
