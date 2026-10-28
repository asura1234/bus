use super::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Leave a real Unix socket between bind and listen, exactly the window that
/// a pathname-based readiness check mistakes for a serving driver.
fn bound_socket(path: &Path) -> OwnedFd {
    // SAFETY: a successful socket call returns a new descriptor owned here.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    assert!(fd >= 0);
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    let bytes = path.as_os_str().as_bytes();
    assert!(bytes.len() < address.sun_path.len());
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as _;
    }
    // SAFETY: address contains a terminated pathname and fd is a live socket.
    assert_eq!(
        unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                std::mem::size_of_val(&address) as _,
            )
        },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
    fd
}

#[test]
fn failed_start_removes_host_and_scratch_state_when_driver_cannot_listen() {
    let name = session_name("bindfail");
    let cleanup = Session(name.clone());
    let mut start = bus(&name);
    start
        .args(["--dev", "tui", "start", "--name", &name, "--run-dir"])
        .arg(run_dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = start.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !session_dir(&name).join("bus-tui-session").exists() {
        assert!(
            Instant::now() < deadline,
            "start did not claim its scratch directory"
        );
        std::thread::yield_now();
    }
    let _bound = bound_socket(&session_dir(&name).join("d.sock"));
    let (code, response) = parse(child.wait_with_output().unwrap());
    // Save the PID before asserting, so even red-on-base evidence cannot leak
    // the host or its Bus. A failed bind makes the host tear itself down.
    let info = std::fs::read_to_string(session_dir(&name).join("session.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if let Some(pid) = info.as_ref().and_then(|value| value["host_pid"].as_u64()) {
        while unsafe { libc::kill(pid as _, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { libc::kill(pid as _, 0) },
            0,
            "failed-start host {pid} survived"
        );
    }
    let removed = !session_dir(&name).exists();
    if !removed {
        // Only this test's claimed, exited fixture is removed on the old code.
        let _ = std::fs::remove_dir_all(session_dir(&name));
    }
    drop(cleanup);
    assert_eq!(code, 1, "failed start must use start_failed: {response}");
    assert_eq!(response["error"]["code"], "start_failed", "{response}");
    assert!(
        removed,
        "failed start left its scratch state behind: {response}"
    );
}

fn repeated_starts(tag: &str) {
    for attempt in 0..10 {
        let session = Session(session_name(tag));
        let (code, response) = tui(
            &session.0,
            &["start", "--run-dir", run_dir().to_str().unwrap()],
        );
        assert_eq!(code, 0, "startup attempt {attempt}/10: {response}");
        assert_eq!(response["alive"], true, "{response}");
        assert_eq!(session.ok(&["status"])["host_pid"], response["host_pid"]);
    }
}

macro_rules! startup_stress {
    ($($case:ident => $tag:literal),+ $(,)?) => {$(
        #[test]
        fn $case() { repeated_starts($tag); }
    )+};
}

startup_stress! {
    repeated_start_ready_1 => "ready1",
    repeated_start_ready_2 => "ready2",
    repeated_start_ready_3 => "ready3",
    repeated_start_ready_4 => "ready4",
    repeated_start_ready_5 => "ready5",
    repeated_start_ready_6 => "ready6",
    repeated_start_ready_7 => "ready7",
    repeated_start_ready_8 => "ready8",
}
