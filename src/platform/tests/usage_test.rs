use super::*;

#[cfg(target_os = "macos")]
#[test]
fn macos_samples_named_threads_without_waiting_for_their_loop() {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("perf-cpu-probe".into())
        .spawn(move || {
            ready_tx.send(()).unwrap();
            let mut spin = 0_u64;
            while stop_rx.try_recv().is_err() {
                spin = std::hint::black_box(spin.wrapping_add(1));
            }
        })
        .unwrap();
    ready_rx.recv().unwrap();
    let first = self_usage().unwrap().thread_cpu;
    std::thread::sleep(Duration::from_millis(30));
    let second = self_usage().unwrap().thread_cpu;
    stop_tx.send(()).unwrap();
    worker.join().unwrap();
    let first = first.expect("per-thread sample on macOS");
    let second = second.expect("per-thread sample on macOS");
    let before = first.iter().find(|t| t.name == "perf-cpu-probe").unwrap();
    let after = second.iter().find(|t| t.id == before.id).unwrap();
    assert!(
        after.cpu_time > before.cpu_time,
        "CPU advances while the worker remains inside one call"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn self_usage_reports_this_process() {
    let busy_until = std::time::Instant::now() + std::time::Duration::from_millis(30);
    let mut spin = 0_u64;
    while std::time::Instant::now() < busy_until {
        spin = std::hint::black_box(spin.wrapping_add(1));
    }
    let _file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();

    let usage = self_usage().expect("usage on unix");
    assert!(
        usage.cpu_time >= std::time::Duration::from_millis(10),
        "{usage:?}"
    );
    assert!(usage.rss_bytes > 1024 * 1024, "{usage:?}");
    assert!(usage.threads >= 1, "{usage:?}");
    assert!(usage.open_fds >= 4, "stdio plus the open file: {usage:?}");
    assert!(usage.load_1m.is_some_and(|load| load >= 0.0), "{usage:?}");
    assert!(usage.cpus >= 1, "{usage:?}");
    let (busy, total) = usage.system_cpu_ticks.expect("system CPU ticks");
    assert!(total > 0 && busy <= total, "{usage:?}");
    assert!(usage.swap_used_bytes.is_some(), "{usage:?}");
}

#[cfg(target_os = "macos")]
#[test]
fn stack_capture_command_samples_the_pid_into_the_file() {
    let command = stack_capture_command(42, std::path::Path::new("/tmp/out.txt"));
    assert_eq!(command.get_program(), "/usr/bin/sample");
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["42", "3", "-mayDie", "-file", "/tmp/out.txt"]);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn stack_capture_is_unsupported_off_macos() {
    assert!(!capture_stacks(
        std::process::id(),
        std::path::Path::new("/tmp/unused.txt")
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn stack_capture_reports_failure_when_the_pid_cannot_be_sampled() {
    let mut exited = std::process::Command::new("/usr/bin/true")
        .spawn()
        .expect("spawn");
    let pid = exited.id();
    exited.wait().expect("wait");
    let dir = crate::utils::test_temp::unique_temp_path("bus-stack-capture-dead-pid");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("stall.txt");

    let ok = capture_stacks(pid, &out);
    std::thread::sleep(std::time::Duration::from_secs(2));
    let written = out.exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!written, "sample cannot attach to an exited pid");
    assert!(
        !ok,
        "a capture that wrote no stacks must not be reported as ok"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn open_fds_counts_only_the_descriptors_that_are_open() {
    fn listed_fds() -> u32 {
        // The directory read holds one descriptor of its own.
        (std::fs::read_dir("/dev/fd").unwrap().count() as u32).saturating_sub(1)
    }
    let _held: Vec<_> = (0..10)
        .map(|_| std::fs::File::open(std::env::current_exe().unwrap()).unwrap())
        .collect();
    let actual = listed_fds();
    let reported = open_fds().expect("open fds on macOS");
    assert!(
        reported <= actual + 2,
        "reported {reported} open fds, /dev/fd lists {actual}"
    );
}

#[cfg(target_os = "macos")]
#[test]
#[allow(deprecated)]
fn process_sampling_releases_host_port_send_rights() {
    unsafe extern "C" {
        fn mach_port_get_refs(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
            right: u32,
            refs: *mut u32,
        ) -> libc::kern_return_t;
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    let host = unsafe { libc::mach_host_self() };
    let refs = || {
        let mut count = 0;
        assert_eq!(
            unsafe { mach_port_get_refs(libc::mach_task_self(), host, 0, &mut count) },
            0
        );
        count
    };
    let before = refs();
    for _ in 0..32 {
        assert!(self_usage().is_some());
    }
    let after = refs();
    unsafe {
        mach_port_deallocate(libc::mach_task_self(), host);
    }
    assert_eq!(
        after, before,
        "sampling must balance each mach_host_self send right"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_machine_sample_reports_process_cpu_in_nanoseconds() {
    fn own_cpu_ns() -> u64 {
        let pid = std::process::id();
        super::super::machine_usage::sample()
            .unwrap()
            .processes
            .into_iter()
            .find(|process| process.pid == pid)
            .unwrap()
            .cpu_ns
    }
    let (machine_before, usage_before) = (own_cpu_ns(), self_usage().unwrap().cpu_time);
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    let mut spin = 0_u64;
    while std::time::Instant::now() < deadline {
        spin = std::hint::black_box(spin.wrapping_add(1));
    }
    let machine_ns = own_cpu_ns() - machine_before;
    let usage_ns = (self_usage().unwrap().cpu_time - usage_before).as_nanos() as u64;
    assert!(
        machine_ns >= usage_ns / 2 && machine_ns <= usage_ns.saturating_mul(2),
        "machine sample cpu_ns grew {machine_ns} while this process used {usage_ns} ns of CPU"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_machine_sample_accounts_for_unreadable_processes() {
    let sample = super::super::machine_usage::sample().unwrap();
    assert!(sample.enumerated >= sample.processes.len());
    assert_eq!(
        sample.enumerated,
        sample.processes.len() + sample.unavailable
    );
    assert!(
        sample
            .processes
            .iter()
            .any(|process| process.pid == std::process::id()),
        "the readable-process scope must include this process"
    );
}
