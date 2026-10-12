use super::*;

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
