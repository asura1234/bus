//! Unix ownership primitives for a supervised child process group.
use std::{
    fs, io,
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn acquire_writer_lock(path: &Path) -> io::Result<fs::File> {
    use std::os::fd::AsRawFd;
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Writer lock is not a regular file"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

pub(crate) fn supervised_command(argv: &[String], lock: &fs::File) -> Command {
    // The lifetime watcher keeps the original pipe, while the provider reads
    // /dev/null. Either provider exit or Bus EOF tears down the whole group.
    const SUPERVISOR: &str = r#"
cleanup() {
    trap '' HUP INT TERM
    kill -TERM 0 2>/dev/null
    /bin/sleep 0.3
    kill -KILL 0 2>/dev/null
}
trap cleanup EXIT HUP INT TERM
"$@" </dev/null &
provider=$!
( IFS= read -r lifetime || :; kill -TERM "$$" 2>/dev/null ) <&0 &
wait "$provider"
"#;
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", SUPERVISOR, "bus-owned-process"])
        .args(argv)
        .stdin(Stdio::piped())
        .process_group(0);
    inherit_writer_lock(&mut command, lock);
    command
}

pub(crate) fn inherit_writer_lock(command: &mut Command, lock: &fs::File) {
    use std::os::fd::AsRawFd;
    let fd = lock.as_raw_fd();
    // F_DUPFD allocates a free descriptor with CLOEXEC cleared. It never
    // overwrites std::process's own exec-error pipe or any inherited handle.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_DUPFD, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// A lifetime socket carries no payload. Readability, including HUP, is EOF.
pub(crate) fn ensure_owner_alive(control: &UnixStream) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: control.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if result > 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "Process owner stopped",
        ));
    }
    Ok(())
}

/// Retire the entire private group, including tools that outlive its leader.
pub(crate) fn terminate_group(child: &mut Child, timeout: Duration) -> io::Result<()> {
    let group = -(child.id() as i32);
    unsafe {
        libc::kill(group, libc::SIGTERM);
    }
    let started = Instant::now();
    loop {
        child.try_wait()?;
        if unsafe { libc::kill(group, 0) } != 0 {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Process group has not retired",
            ));
        }
        if started.elapsed() >= Duration::from_millis(300) {
            unsafe {
                libc::kill(group, libc::SIGKILL);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
#[path = "tests/owned_process_test.rs"]
mod tests;
