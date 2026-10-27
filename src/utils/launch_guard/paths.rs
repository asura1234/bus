//! Path validation shared by test launch guards and pre-spawn fixture setup.
use std::{
    io,
    path::{Path, PathBuf},
};

pub(crate) const PROTECTED_HOME: &str = "TEST_BUS_PROTECTED_HOME";

fn resolved(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::other(
            "test isolation refused: test paths must be absolute",
        ));
    }
    if path.exists() {
        return path.canonicalize();
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("test isolation refused: missing path parent"))?;
    let tail = path
        .file_name()
        .ok_or_else(|| io::Error::other("test isolation refused: invalid test path"))?;
    Ok(resolved(parent)?.join(tail))
}

pub(crate) fn validate(home: &Path, data: Option<&Path>, protected: &Path) -> io::Result<()> {
    let protected = resolved(protected)?;
    if resolved(home)? == protected {
        return Err(io::Error::other(
            "test isolation refused: set a scratch HOME before booting Bus",
        ));
    }
    if let Some(root) = data {
        if root.is_absolute() && resolved(root)?.starts_with(protected.join(".local/share/bus")) {
            return Err(io::Error::other(
                "test isolation refused: BUS_DATA_DIR points at the protected HOME's Bus data",
            ));
        }
    }
    Ok(())
}

/// A PTY chooses HOME as its initial cwd before the child can run its guard.
#[cfg(test)]
pub(crate) fn prepare_fixture_home(home: &Path) -> io::Result<()> {
    let protected = std::env::var_os(PROTECTED_HOME)
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
        .ok_or_else(|| io::Error::other("test isolation refused: protected HOME is unavailable"))?;
    validate(home, None, &protected)?;
    std::fs::create_dir_all(home)
}

#[cfg(test)]
#[path = "../tests/launch_paths_test.rs"]
mod tests;
