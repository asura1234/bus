//! Stops a test-spawned bus without truncating its coverage profile. Shared by
//! `process_test.rs` and test crates that do not include it, such as room-screen.
use portable_pty::{Child, MasterPty};
#[cfg(unix)]
use std::time::{Duration, Instant};

/// How long a stopped bus may take to finish its graceful exit before teardown
/// treats it as hung.
#[cfg(unix)]
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(30);

/// Stops a spawned bus like a closing terminal does: SIGHUP and close its PTY,
/// then wait until it has exited. portable-pty's `Child::kill` sends SIGKILL
/// about 200 ms after its SIGHUP, which lands inside the bus's graceful exit on
/// a busy machine and truncates the coverage profile it writes last, failing the
/// coverage merge. The PTY is closed before waiting because an exiting process
/// cannot finish while its unread terminal output is still draining. SIGKILL is
/// kept for a bus that is still running after `STOP_GRACE`.
pub fn stop_spawned_bus(
    child: &mut (dyn Child + Send + Sync),
    master: Option<Box<dyn MasterPty + Send>>,
) {
    #[cfg(unix)]
    if let (Some(pid), Ok(None)) = (child.process_id(), child.try_wait()) {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGHUP);
        }
        drop(master);
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline {
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!("bus {pid} still running {STOP_GRACE:?} after SIGHUP; sending SIGKILL");
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
        return;
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    drop(master);
}
