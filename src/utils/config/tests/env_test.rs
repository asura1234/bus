//! Shared test-only environment synchronization; no component dependency.
use std::sync::{Mutex, MutexGuard, OnceLock};

pub(crate) fn mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) struct EnvLock {
    _guard: MutexGuard<'static, ()>,
    #[cfg(windows)]
    appdata: Option<std::ffi::OsString>,
}

impl Drop for EnvLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(appdata) = self.appdata.take() {
            std::env::set_var("APPDATA", appdata);
        } else {
            std::env::remove_var("APPDATA");
        }
    }
}

pub(crate) fn env_lock() -> EnvLock {
    let guard = mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    EnvLock {
        _guard: guard,
        #[cfg(windows)]
        appdata: std::env::var_os("APPDATA"),
    }
}

#[cfg(not(windows))]
#[test]
fn tilde_path_with_repeated_separator_stays_relative_to_home() {
    let _lock = mutex().lock().unwrap();
    let previous = std::env::var_os("HOME");
    let home = std::path::PathBuf::from("/tmp/bus-tilde-home");
    std::env::set_var("HOME", &home);

    let expanded = crate::utils::home_path::expand_tilde_path("~//projects/bus");

    match previous {
        Some(previous) => std::env::set_var("HOME", previous),
        None => std::env::remove_var("HOME"),
    }
    assert_eq!(expanded, home.join("projects/bus"));
}
