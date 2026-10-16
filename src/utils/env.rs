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

#[cfg(test)]
#[path = "tests/env_facts_test.rs"]
mod tests;
