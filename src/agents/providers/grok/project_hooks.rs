//! One transaction owns global isolation for both installation and rebinding.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Install,
    Resume,
}

#[cfg(unix)]
mod exclude;
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
        let result = unix::update(project, binary, mode, home, validate)?;
        if let Some(root) = result
            .0
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        {
            if let Err(error) = exclude::ensure(root) {
                tracing::warn!(root = %root.display(), %error,
                    "Could not exclude Bus-generated Grok hooks from Git status");
            }
        }
        Ok(result)
    }
    #[cfg(not(unix))]
    {
        let _ = (project, binary, mode, home, validate);
        Err("Grok project hooks require no-follow directory-relative I/O on this platform".into())
    }
}
