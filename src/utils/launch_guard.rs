//! Test subprocesses carry the HOME that they must never read or write.
use std::{io, path::Path};

mod paths;

/// Check before the offer, settings, registry or hidden daemon setup.
pub(crate) fn guard_launch() -> io::Result<()> {
    let Some(protected) = std::env::var_os(paths::PROTECTED_HOME) else {
        return Ok(());
    };
    let home = std::env::home_dir()
        .ok_or_else(|| io::Error::other("test isolation refused: HOME is unavailable"))?;
    let data = super::env::bus_data_dir();
    paths::validate(&home, data.as_deref(), Path::new(&protected))
}
