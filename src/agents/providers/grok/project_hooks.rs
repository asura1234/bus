//! One transaction owns global isolation for both installation and rebinding.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Install,
    Resume,
}

#[cfg(unix)]
mod unix;

pub(super) fn update<T>(
    project: &Path,
    binary: &Path,
    mode: Mode,
    home: Option<&Path>,
    validate: impl FnOnce() -> Result<T, String>,
) -> Result<(PathBuf, bool, T, PathBuf), String> {
    #[cfg(unix)]
    {
        unix::update(project, binary, mode, home, validate)
    }
    #[cfg(not(unix))]
    {
        let _ = (project, binary, mode, home, validate);
        Err("Grok project hooks require no-follow directory-relative I/O on this platform".into())
    }
}
