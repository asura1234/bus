//! This process's resource usage and stack captures, for the perf recorder.
//!
//! These return raw facts; the recorder turns deltas into percentages.
use std::path::Path;
use std::time::Duration;

use crate::utils::perf::{PerfHooks, ProcessUsage};

/// The recorder's view of this platform.
pub(crate) fn perf_hooks() -> PerfHooks {
    PerfHooks {
        sample: self_usage,
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

#[cfg(target_os = "macos")]
fn open_fds() -> Option<u32> {
    // With no buffer, the call returns the bytes needed for every open descriptor.
    let bytes = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDLISTFDS,
            0,
            std::ptr::null_mut(),
            0,
        )
    };
    (bytes > 0).then(|| bytes as u32 / std::mem::size_of::<libc::proc_fdinfo>() as u32)
}

#[cfg(target_os = "macos")]
fn system_cpu_ticks() -> Option<(u64, u64)> {
    let mut info: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    let result = unsafe {
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        libc::host_statistics(
            host,
            libc::HOST_CPU_LOAD_INFO,
            &mut info as *mut _ as libc::host_info_t,
            &mut count,
        )
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

/// Starts a background stack capture of `pid` into `out`; macOS only.
#[cfg(target_os = "macos")]
pub(crate) fn capture_stacks(pid: u32, out: &Path) -> bool {
    let mut command = stack_capture_command(pid, out);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => std::thread::Builder::new()
            .name("bus-perf-capture".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .is_ok(),
        Err(_) => false,
    }
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
