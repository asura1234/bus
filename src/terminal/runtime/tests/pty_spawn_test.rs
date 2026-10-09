use crate::terminal::runtime::spawn::spawn_with_portable_pty;
use portable_pty::CommandBuilder;
use std::sync::{Mutex, OnceLock};

fn pty_fd_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn parent_pty_fd_targets() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return Vec::new();
    };
    let mut targets: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .map(|target| target.to_string_lossy().into_owned())
        .filter(|target| target.starts_with("/dev/pts/") || target == "/dev/ptmx")
        .collect();
    targets.sort();
    targets
}

fn parent_pty_fd_count() -> usize {
    parent_pty_fd_targets().len()
}

#[test]
fn portable_pty_setup_leaves_one_parent_pty_fd() {
    let _guard = pty_fd_test_lock().lock().expect("pty fd test lock");
    let before = parent_pty_fd_count();
    let mut cmd = CommandBuilder::new("/bin/cat");
    cmd.env(
        crate::utils::env::HERDR_ENV_VAR,
        crate::utils::env::HERDR_ENV_VALUE,
    );

    let mut spawned = spawn_with_portable_pty(24, 80, cmd).expect("portable pty setup succeeds");
    let after_spawn = parent_pty_fd_count();

    assert_eq!(
        after_spawn,
        before + 1,
        "portable-pty setup should leave only the Herdr-owned master fd in the parent: {:?}",
        parent_pty_fd_targets()
    );

    let _ = spawned.child.kill();
    let _ = spawned.child.wait();
    drop(spawned.master_fd);
}
