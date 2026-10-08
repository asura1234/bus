#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock};

use portable_pty::CommandBuilder;

pub(crate) const HERDR_PANE_ID_ENV_VAR: &str = "HERDR_PANE_ID";
pub(crate) const HERDR_TAB_ID_ENV_VAR: &str = "HERDR_TAB_ID";
pub(crate) const HERDR_WORKSPACE_ID_ENV_VAR: &str = "HERDR_WORKSPACE_ID";

pub(crate) fn apply_pane_base_env(cmd: &mut CommandBuilder) {
    cmd.env(
        crate::protocol::api::SOCKET_PATH_ENV_VAR,
        crate::protocol::api::socket_path(),
    );
    if let Ok(executable) = std::env::current_exe() {
        cmd.env("HERDR_BIN_PATH", executable);
    }
}

/// Serializes tests that mutate process environment variables such as HOME or APPDATA.
#[cfg(test)]
pub(crate) struct EnvLock {
    _guard: MutexGuard<'static, ()>,
    #[cfg(windows)]
    appdata: Option<std::ffi::OsString>,
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn env_lock() -> EnvLock {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    EnvLock {
        _guard: guard,
        #[cfg(windows)]
        appdata: std::env::var_os("APPDATA"),
    }
}
