//! Inherited process facts shared by the component callers.
use std::path::PathBuf;

pub(crate) const BUS_ENV_VAR: &str = "HERDR_ENV";
pub(crate) const BUS_ENV_VALUE: &str = "1";

pub const SOCKET_PATH_ENV_VAR: &str = "HERDR_SOCKET_PATH";
pub(crate) const STARTUP_CWD_ENV_VAR: &str = "HERDR_STARTUP_CWD";
pub(crate) const STOP_WITHOUT_CLIENTS_ENV_VAR: &str = "HERDR_STOP_WITHOUT_CLIENTS";
/// Private startup handoff to the daemon, never a saved preference.
pub(crate) const YOLO_ENV_VAR: &str = "BUS_YOLO";

pub(crate) fn yolo_enabled() -> bool {
    std::env::var_os(YOLO_ENV_VAR).is_some_and(|value| value == "1")
}

/// Preserve an inherited root exactly, including empty and non-UTF8 values.
pub(crate) fn bus_data_dir() -> Option<PathBuf> {
    std::env::var_os("BUS_DATA_DIR").map(PathBuf::from)
}

/// PATH for in-process lookups. Lib tests share one process, so a thread-local
/// override replaces `set_var("PATH")`, which would change every other test.
pub(crate) fn path_var() -> Option<std::ffi::OsString> {
    #[cfg(test)]
    if let Some(path) = path_override_value() {
        return Some(path);
    }
    std::env::var_os("PATH")
}

/// First `program` on [`path_var`], if it is a file. Relative entries stay
/// relative; callers that exec after a chdir resolve them.
pub(crate) fn resolve_on_path(program: impl AsRef<std::ffi::OsStr>) -> Option<PathBuf> {
    let program = program.as_ref();
    let path = path_var()?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// `Command` for a bare program name. An active test override never falls
/// back to the process PATH, or a fixture directory would still run the real tool.
pub(crate) fn command_on_path(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let program = program.as_ref();
    if let Some(resolved) = resolve_on_path(program) {
        return std::process::Command::new(resolved);
    }
    if path_override_active() {
        return std::process::Command::new(
            std::env::temp_dir().join("bus-path-miss").join(program),
        );
    }
    std::process::Command::new(program)
}

fn path_override_active() -> bool {
    #[cfg(test)]
    {
        PATH_OVERRIDE.with(|slot| slot.borrow().is_some())
    }
    #[cfg(not(test))]
    {
        false
    }
}

#[cfg(test)]
thread_local! {
    static PATH_OVERRIDE: std::cell::RefCell<Option<std::ffi::OsString>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn path_override_value() -> Option<std::ffi::OsString> {
    PATH_OVERRIDE.with(|slot| slot.borrow().clone())
}

/// Restores the previous override, including "no override", on drop.
#[cfg(test)]
pub(crate) struct PathOverride {
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl PathOverride {
    pub(crate) fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = PATH_OVERRIDE.with(|slot| slot.replace(Some(path.as_ref().to_os_string())));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for PathOverride {
    fn drop(&mut self) {
        let previous = self.previous.take();
        PATH_OVERRIDE.with(|slot| {
            *slot.borrow_mut() = previous;
        });
    }
}

/// The calling thread's test PATH overlay, moved onto a worker.
/// Playback and other spawned work resolve programs after `spawn`, where a
/// thread-local overlay would otherwise be empty and the real PATH would win.
pub(crate) struct InheritedPathOverride {
    #[cfg(test)]
    path: Option<std::ffi::OsString>,
}

pub(crate) fn capture_path_override() -> InheritedPathOverride {
    InheritedPathOverride {
        #[cfg(test)]
        path: path_override_value(),
    }
}

pub(crate) struct InstalledPathOverride {
    #[cfg(test)]
    _guard: Option<PathOverride>,
}

impl InheritedPathOverride {
    /// Install on this thread. Hold the result until the worker finishes.
    pub(crate) fn install(self) -> InstalledPathOverride {
        #[cfg(test)]
        let _guard = self.path.map(PathOverride::set);
        #[cfg(not(test))]
        let _ = self;
        InstalledPathOverride {
            #[cfg(test)]
            _guard,
        }
    }
}

#[cfg(test)]
#[path = "tests/env_facts_test.rs"]
mod tests;
