//! Quit and restart must leave one TUI. The previous TUI pid dies, SIGTERM is
//! enough to reap a stuck cleanup, and a closed tty is not ignored.

#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Session {
    root: PathBuf,
    master: RawFd,
    child: std::process::Child,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            libc::killpg(self.child.id() as i32, libc::SIGKILL);
            if self.master >= 0 {
                libc::close(self.master);
            }
        }
        if let Some(pid) = server_pid(self) {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn session() -> Session {
    open_session(&[])
}

fn open_session(extra_env: &[(&str, &str)]) -> Session {
    // The shared allocator stays under macOS's Unix-socket path limit because
    // its root is /tmp, unlike `std::env::temp_dir()`.
    let root = crate::fixture_dirs::unique_temp_path("life");
    fs::create_dir_all(&root).unwrap();
    let mut perms = fs::metadata(&root).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&root, perms.clone()).unwrap();
    for name in ["data", "home", "project"] {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, perms.clone()).unwrap();
    }
    let mut master = 0;
    let mut slave = 0;
    let mut size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
    // openpty's master is inheritable. The supervisor would keep that fd, so
    // closing it in this test would not be the last reference and the slave
    // would never see hangup.
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) },
        0,
        "master cloexec: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(
        unsafe { libc::ioctl(slave, libc::TIOCSWINSZ, &size) },
        0,
        "set window size: {}",
        std::io::Error::last_os_error()
    );
    let binary = env!("CARGO_BIN_EXE_bus");
    let data = root.join("data");
    let home = root.join("home");
    let mut command = Command::new(binary);
    command
        .args(["--dev", "--yolo"])
        .current_dir(root.join("project"))
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("BUS_DATA_DIR", &data)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("TERM", "xterm-256color")
        .env("TERM_PROGRAM", "ghostty")
        .env("SHELL", "/bin/sh")
        .env("USER", "tester")
        .env("TMPDIR", "/tmp")
        .env("LANG", "C");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
        .stdin(unsafe { Stdio::from_raw_fd(libc::dup(slave)) })
        .stdout(unsafe { Stdio::from_raw_fd(libc::dup(slave)) })
        .stderr(unsafe { Stdio::from_raw_fd(libc::dup(slave)) })
        .process_group(0);
    unsafe {
        libc::close(slave);
    }
    let child = command.spawn().expect("spawn bus");
    let master_fd = master;
    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&output);
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        unsafe {
            let flags = libc::fcntl(master_fd, libc::F_GETFL);
            libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        loop {
            let read = unsafe { libc::read(master_fd, buf.as_mut_ptr().cast(), buf.len()) };
            if read > 0 {
                captured
                    .lock()
                    .expect("output")
                    .extend_from_slice(&buf[..read as usize]);
                continue;
            }
            if read == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            break;
        }
    });
    Session {
        root,
        master,
        child,
        output,
    }
}

fn screen(session: &Session) -> String {
    let bytes = session.output.lock().expect("output").clone();
    String::from_utf8_lossy(&bytes)
        .chars()
        .rev()
        .take(800)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn data(session: &Session) -> PathBuf {
    session.root.join("data")
}

fn server_pid(session: &Session) -> Option<u32> {
    let text = fs::read_to_string(data(session).join("restarts/build.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["pid"].as_u64().map(|pid| pid as u32)
}

fn tui_pid(session: &Session) -> Option<u32> {
    let path = data(session).join("restarts/tui.json");
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["process"]["pid"].as_u64().map(|pid| pid as u32)
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// A zombie still answers `kill(0)` until its parent waits. The shell reaps
/// the supervisor as soon as it exits; this test is that parent, so a zombie
/// supervisor is already gone.
fn process_stat(pid: u32) -> String {
    Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_default()
}

fn running(pid: u32) -> bool {
    let stat = process_stat(pid);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn process_tree(pid: u32) -> String {
    Command::new("ps")
        .args(["-ax", "-o", "pid,ppid,stat,wchan,command"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| {
            text.lines()
                .filter(|line| line.contains(&pid.to_string()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn wait_until(seconds: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn stdin_path(pid: u32) -> String {
    let output = Command::new("lsof")
        .args([
            "-n",
            "-P",
            "-a",
            "-p",
            &pid.to_string(),
            "-d",
            "0",
            "-F",
            "n",
        ])
        .output()
        .expect("lsof");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix('n'))
        .unwrap_or("")
        .to_string()
}

fn sorted_tty_readers(supervisor: u32, binary: &str) -> Vec<u32> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .output()
        .unwrap();
    let mut parent = HashMap::new();
    let mut readers = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let Some(pid) = parts.next().and_then(|text| text.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ppid) = parts.next().and_then(|text| text.parse::<u32>().ok()) else {
            continue;
        };
        parent.insert(pid, ppid);
        let command = parts.collect::<Vec<_>>().join(" ");
        if !command.contains(binary) || command.contains(" server") {
            continue;
        }
        if stdin_path(pid).starts_with("/dev/tty") && descends_from(pid, supervisor, &parent) {
            readers.push(pid);
        }
    }
    readers.sort_unstable();
    readers
}

fn descends_from(mut pid: u32, ancestor: u32, parent: &HashMap<u32, u32>) -> bool {
    for _ in 0..8 {
        if pid == ancestor {
            return true;
        }
        let Some(next) = parent.get(&pid).copied() else {
            return false;
        };
        if next == 0 || next == pid {
            return false;
        }
        pid = next;
    }
    false
}

fn restart(session: &Session) {
    let log = data(session).join("restarts/log.jsonl");
    let before = fs::read_to_string(&log)
        .map(|text| text.lines().count())
        .unwrap_or(0);
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args([
            "--dev",
            "restart",
            "--binary",
            env!("CARGO_BIN_EXE_bus"),
            "--confirm",
        ])
        .env_clear()
        .env("HOME", session.root.join("home"))
        .env("USERPROFILE", session.root.join("home"))
        .env("BUS_DATA_DIR", data(session))
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("USER", "tester")
        .output()
        .expect("restart");
    assert!(
        output.status.success(),
        "restart failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        wait_until(45, || {
            fs::read_to_string(&log).ok().is_some_and(|text| {
                text.lines().count() > before
                    && text
                        .lines()
                        .last()
                        .is_some_and(|line| line.contains("\"restarted\""))
            })
        }),
        "restart outcome missing: {}",
        fs::read_to_string(&log).unwrap_or_default()
    );
}

#[test]
fn restart_handoff_reaps_the_previous_tui_and_a_second_restart_leaves_one() {
    let binary = env!("CARGO_BIN_EXE_bus");
    let session = session();
    assert!(
        wait_until(30, || tui_pid(&session).is_some()),
        "tui.json was not written; screen {}",
        screen(&session)
    );
    let first = tui_pid(&session).unwrap();
    let supervisor = session.child.id();
    assert_ne!(
        first, supervisor,
        "the shell-facing process must not be the TUI"
    );
    assert!(alive(first));
    restart(&session);
    assert!(
        wait_until(3, || !alive(first)),
        "previous TUI {first} still alive after handoff"
    );
    let second = tui_pid(&session).expect("replacement tui.json");
    assert_ne!(second, first);
    assert!(alive(second));
    assert_eq!(sorted_tty_readers(supervisor, binary), vec![second]);
    restart(&session);
    assert!(
        wait_until(3, || !alive(second)),
        "second TUI {second} still alive after the next handoff"
    );
    let third = tui_pid(&session).expect("third tui.json");
    assert!(alive(third));
    assert_eq!(sorted_tty_readers(supervisor, binary), vec![third]);
    assert!(alive(supervisor));
    assert!(!stdin_path(supervisor).starts_with("/dev/tty"));
}

#[test]
fn sigterm_reaps_the_tui_without_sigkill() {
    let session = session();
    assert!(wait_until(30, || tui_pid(&session).is_some()));
    let tui = tui_pid(&session).unwrap();
    assert_eq!(unsafe { libc::kill(tui as i32, libc::SIGTERM) }, 0);
    let supervisor = session.child.id();
    assert!(
        wait_until(8, || !running(tui) && !running(supervisor)),
        "SIGTERM left tui={tui} stat={:?} supervisor={supervisor} stat={:?}",
        process_stat(tui),
        process_stat(supervisor)
    );
}

#[test]
fn closed_tty_reaps_the_tui() {
    let mut session = session();
    assert!(wait_until(30, || tui_pid(&session).is_some()));
    let tui = tui_pid(&session).unwrap();
    unsafe {
        libc::close(session.master);
    }
    session.master = -1;
    let supervisor = session.child.id();
    assert!(
        wait_until(8, || !running(tui) && !running(supervisor)),
        "closed tty left tui={tui} stat={:?} supervisor={supervisor} stat={:?}\nps:\n{}\nlog:\n{}",
        process_stat(tui),
        process_stat(supervisor),
        process_tree(supervisor),
        client_log(&session)
    );
}

#[test]
fn forced_quit_reaps_the_tui() {
    let session = open_session(&[
        ("BUS_QUIT_SAVE_TIMEOUT_MS", "1"),
        // Shutdown is in-process and otherwise acks in a few milliseconds,
        // before the 100ms stdin poll can deliver the confirming key.
        ("BUS_QUIT_SHUTDOWN_DELAY_MS", "400"),
    ]);
    assert!(wait_until(30, || tui_pid(&session).is_some()));
    unsafe {
        assert_eq!(libc::write(session.master, b"\x11".as_ptr().cast(), 1), 1);
    }
    assert!(
        wait_until(3, || client_log(&session).contains("bus.quit.warning")),
        "force window did not open: {}",
        client_log(&session)
    );
    unsafe {
        assert_eq!(libc::write(session.master, b"\x11".as_ptr().cast(), 1), 1);
    }
    assert!(
        wait_until(3, || client_log(&session).contains("bus.quit.forced")),
        "forced quit did not record bus.quit.forced: {}",
        client_log(&session)
    );
    let tui = tui_pid(&session).unwrap();
    let supervisor = session.child.id();
    assert!(
        wait_until(8, || !running(tui) && !running(supervisor)),
        "forced quit left tui={tui} stat={:?} supervisor={supervisor} stat={:?}; log {}",
        process_stat(tui),
        process_stat(supervisor),
        client_log(&session)
    );
}

fn client_log(session: &Session) -> String {
    let mut found = String::new();
    let root = data(session);
    let _ = visit_logs(&root, &mut found);
    found
}

fn visit_logs(path: &Path, found: &mut String) -> std::io::Result<()> {
    if path.is_file() && path.file_name().and_then(|name| name.to_str()) == Some("herdr-client.log")
    {
        found.push_str(&fs::read_to_string(path).unwrap_or_default());
        return Ok(());
    }
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let _ = visit_logs(&entry?.path(), found);
        }
    }
    Ok(())
}
