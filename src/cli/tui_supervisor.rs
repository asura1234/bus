//! The shell waits on this process. The TUI is always a child, so a restart can
//! exit that child without returning the terminal to the shell.
//!
//! Quit used to `wait` on the replacement inside the process that had just been
//! the TUI. That pid stayed alive on the same tty, its stdin reader still
//! blocked in `read`, and `ctrlc`'s SIGTERM handler only set a flag nobody
//! read, so the process ignored SIGTERM until SIGKILL. Nested restarts stacked
//! another waiter each time and those waiters split keyboard input.

use std::ffi::OsString;
use std::io::{self, ErrorKind, Write};
use std::os::fd::RawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use super::session_pick::{parse_invocation, Action};

pub(crate) const SUPERVISOR_ENV: &str = "BUS_TUI_SUPERVISOR";
pub(crate) const SUPERVISED_ENV: &str = "BUS_TUI_SUPERVISED";
pub(crate) const HANDOFF_FD_ENV: &str = "BUS_TUI_HANDOFF_FD";
/// Exit status a TUI uses to ask its supervisor to relaunch from `active.json`.
pub(crate) const RELAUNCH_EXIT: i32 = 86;

static SUPERVISED: AtomicBool = AtomicBool::new(false);
static HANDOFF_FD: AtomicI32 = AtomicI32::new(-1);
static HANDOFF_WRITE: AtomicI32 = AtomicI32::new(-1);
static TTY_IN: AtomicI32 = AtomicI32::new(-1);
static CHILD_PID: AtomicI32 = AtomicI32::new(-1);
static STOP: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_supervised_worker() -> bool {
    SUPERVISED.load(Ordering::Relaxed)
}

/// Remember the supervisor pipe, then drop both markers from the environment
/// so a server or agent spawned later does not inherit them.
pub(crate) fn capture_worker_handoff_fd() {
    if std::env::var(SUPERVISED_ENV).ok().as_deref() == Some("1") {
        SUPERVISED.store(true, Ordering::Relaxed);
        std::env::remove_var(SUPERVISED_ENV);
    }
    let Ok(text) = std::env::var(HANDOFF_FD_ENV) else {
        return;
    };
    std::env::remove_var(HANDOFF_FD_ENV);
    let Ok(fd) = text.parse::<RawFd>() else {
        return;
    };
    // CLOEXEC is per descriptor. The supervisor keeps its own copy open.
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    HANDOFF_FD.store(fd, Ordering::Relaxed);
}

pub(crate) fn should_supervise(args: &[String]) -> bool {
    if is_supervised_worker() || !stdin_is_tty() {
        return false;
    }
    let Ok(invocation) = parse_invocation(args) else {
        return false;
    };
    matches!(invocation.action, Action::Run | Action::Resume(_))
}

pub(crate) fn supervise(args: &[String]) -> io::Result<()> {
    let (read_fd, write_fd) = pipe()?;
    unsafe {
        libc::fcntl(read_fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    HANDOFF_WRITE.store(write_fd, Ordering::Relaxed);
    std::env::set_var(SUPERVISOR_ENV, "1");
    std::env::set_var(HANDOFF_FD_ENV, write_fd.to_string());
    let mut child = spawn_worker(args, write_fd)?;
    park_supervisor_stdin();
    install_stop_signals();
    track_child(&child);
    loop {
        let status = child.wait()?;
        CHILD_PID.store(-1, Ordering::Relaxed);
        if STOP.load(Ordering::Relaxed) || status.code() != Some(RELAUNCH_EXIT) {
            // The TUI restores the terminal. `_exit` skips destructors and
            // stdio flush, either of which can block on a full or closed pty
            // and leave this pid in the process table.
            exit_now(exit_code(&status));
        }
        let root = match read_handoff_root(read_fd) {
            Ok(root) => root,
            Err(error) => {
                restore_tty();
                return Err(error);
            }
        };
        child = match super::restart::relaunch_claimed_handoff(&root) {
            Ok(Some(next)) => next,
            Ok(None) => exit_now(0),
            Err(error) => {
                restore_tty();
                return Err(error);
            }
        };
        track_child(&child);
    }
}

/// Point a relaunched TUI at the original tty and mark it supervised.
pub(crate) fn prepare_relaunch(command: &mut Command) {
    if std::env::var(SUPERVISOR_ENV).ok().as_deref() != Some("1") {
        return;
    }
    command.env(SUPERVISED_ENV, "1");
    command.env_remove(SUPERVISOR_ENV);
    let handoff = HANDOFF_WRITE.load(Ordering::Relaxed);
    if handoff >= 0 {
        command.env(HANDOFF_FD_ENV, handoff.to_string());
    }
    let tty = TTY_IN.load(Ordering::Relaxed);
    if tty < 0 {
        return;
    }
    // `Command` closes the descriptor it owns. Dup so the saved tty survives.
    let duped = unsafe { libc::dup(tty) };
    if duped >= 0 {
        command.stdin(unsafe { Stdio::from_raw_fd(duped) });
    }
}

pub(crate) fn tell_supervisor(root: &Path) -> io::Result<()> {
    let fd = HANDOFF_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return Err(io::Error::other(
            "supervised TUI has no supervisor handoff pipe",
        ));
    }
    let mut bytes = root.as_os_str().as_bytes().to_vec();
    bytes.push(b'\n');
    write_all(fd, &bytes)
}

extern "C" fn on_stop(signal: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
    let pid = CHILD_PID.load(Ordering::Relaxed);
    if pid > 0 {
        unsafe {
            libc::kill(pid, signal);
        }
    }
}

fn track_child(child: &Child) {
    CHILD_PID.store(child.id() as i32, Ordering::Relaxed);
}

fn exit_code(status: &std::process::ExitStatus) -> i32 {
    if status.success() {
        0
    } else {
        status.code().unwrap_or(1)
    }
}

fn exit_now(code: i32) -> ! {
    unsafe { libc::_exit(code) }
}

fn spawn_worker(args: &[String], write_fd: RawFd) -> io::Result<Child> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(args)
        .env(SUPERVISED_ENV, "1")
        .env(HANDOFF_FD_ENV, write_fd.to_string())
        .env_remove(SUPERVISOR_ENV);
    command.spawn()
}

fn park_supervisor_stdin() {
    let saved = unsafe { libc::dup(libc::STDIN_FILENO) };
    if saved < 0 {
        return;
    }
    unsafe {
        libc::fcntl(saved, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    TTY_IN.store(saved, Ordering::Relaxed);
    let null = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
    if null < 0 {
        return;
    }
    unsafe {
        libc::dup2(null, libc::STDIN_FILENO);
        libc::close(null);
    }
}

fn install_stop_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_stop as *const () as libc::sighandler_t);
    }
}

fn pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

fn read_handoff_root(fd: RawFd) -> io::Result<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    while Instant::now() < deadline {
        let read = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        if read == 1 {
            if byte[0] == b'\n' {
                break;
            }
            bytes.push(byte[0]);
            continue;
        }
        if read == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
    if bytes.is_empty() {
        return Err(io::Error::other(
            "supervised TUI exited without a handoff path",
        ));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn write_all(fd: RawFd, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let wrote =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if wrote < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        offset += wrote as usize;
    }
    Ok(())
}

fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

fn restore_tty() {
    let _ = ratatui::try_restore();
    unsafe {
        let flags = libc::fcntl(libc::STDOUT_FILENO, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(libc::STDOUT_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let mut output = io::stdout();
    let _ = output.write_all(
        b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1016l\x1b[?25h\x1b[0 q",
    );
    let _ = output.flush();
}
