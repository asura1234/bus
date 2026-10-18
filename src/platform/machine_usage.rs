//! Bounded macOS process CPU and VM facts. No argv or environment queries.
use crate::utils::perf::machine::MachineSample;
#[cfg(target_os = "macos")]
use crate::utils::perf::machine::ProcessSample;

#[cfg(target_os = "macos")]
pub(crate) fn sample() -> Option<MachineSample> {
    let timebase = cpu_timebase()?;
    const MAX_PIDS: usize = 16384;
    let mut pids = vec![0_i32; MAX_PIDS];
    let bytes = unsafe {
        libc::proc_listpids(
            1, // PROC_ALL_PIDS, from libproc.h; libc does not expose this selector.
            0,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    if bytes <= 0 {
        return None;
    }
    pids.truncate((bytes as usize / std::mem::size_of::<i32>()).min(MAX_PIDS));
    pids.retain(|pid| *pid > 0);
    let enumerated = pids.len();
    let processes: Vec<_> = pids
        .into_iter()
        .filter_map(|pid| process(pid, timebase))
        .collect();
    let unavailable = enumerated - processes.len();
    let mut load = [0.0; 1];
    let load1 = (unsafe { libc::getloadavg(load.as_mut_ptr(), 1) } == 1).then_some(load[0]);
    let mut pressure = 0_u32;
    let mut size = std::mem::size_of_val(&pressure);
    let pressure_ok = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&mut pressure as *mut u32).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    let pages = paging();
    Some(MachineSample {
        processes,
        enumerated,
        unavailable,
        load1,
        memory_pressure: pressure_ok.then_some(pressure),
        page_ins: pages.map(|p| p.0),
        page_outs: pages.map(|p| p.1),
    })
}
#[cfg(target_os = "macos")]
#[allow(deprecated)] // Use the same native Mach ABI as the existing VM sampler.
fn cpu_timebase() -> Option<(u32, u32)> {
    let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
    (unsafe { libc::mach_timebase_info(&mut info) } == libc::KERN_SUCCESS && info.denom != 0)
        .then_some((info.numer, info.denom))
}
#[cfg(target_os = "macos")]
fn process(pid: i32, timebase: (u32, u32)) -> Option<ProcessSample> {
    unsafe fn info<T>(pid: i32, flavor: i32) -> Option<T> {
        let mut result: T = std::mem::zeroed();
        let size = std::mem::size_of::<T>() as i32;
        (libc::proc_pidinfo(pid, flavor, 0, (&mut result as *mut T).cast(), size) == size)
            .then_some(result)
    }
    let bsd: libc::proc_bsdinfo = unsafe { info(pid, libc::PROC_PIDTBSDINFO)? };
    let task: libc::proc_taskinfo = unsafe { info(pid, libc::PROC_PIDTASKINFO)? };
    let basename = bsd
        .pbi_comm
        .iter()
        .take_while(|ch| **ch != 0)
        .map(|ch| *ch as u8)
        .collect::<Vec<_>>();
    Some(ProcessSample {
        pid: pid as u32,
        start_us: bsd
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(bsd.pbi_start_tvusec),
        basename: String::from_utf8_lossy(&basename).into_owned(),
        cpu_ns: ((u128::from(task.pti_total_user.saturating_add(task.pti_total_system))
            * u128::from(timebase.0))
            / u128::from(timebase.1))
        .min(u128::from(u64::MAX)) as u64,
    })
}
#[cfg(target_os = "macos")]
fn paging() -> Option<(u64, u64)> {
    unsafe extern "C" {
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    let mut info: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    #[allow(deprecated)]
    let result = unsafe {
        let host = libc::mach_host_self();
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut info as *mut libc::vm_statistics64).cast(),
            &mut count,
        );
        mach_port_deallocate(libc::mach_task_self(), host);
        result
    };
    (result == libc::KERN_SUCCESS).then_some((info.pageins, info.pageouts))
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn sample() -> Option<MachineSample> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn samples_process_identity_and_paging_without_command_lines() {
        let sample = super::sample().unwrap();
        assert!(sample
            .processes
            .iter()
            .any(|p| p.pid == std::process::id() && p.start_us > 0 && !p.basename.contains('/')));
        assert!(sample.load1.is_some());
        assert!(sample.page_ins.is_some());
        assert!(sample.page_outs.is_some());
    }
}
