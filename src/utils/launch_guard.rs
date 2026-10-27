//! Test subprocesses carry the HOME that they must never read or write.
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

/// Test harnesses set this marker before any test can spawn a non-test Bus
/// binary. Check before the offer, settings, registry or hidden daemon setup.
pub(crate) fn guard_launch() -> io::Result<()> {
    let Some(protected) = std::env::var_os(PROTECTED_HOME) else {
        return Ok(());
    };
    let protected = resolved(Path::new(&protected))?;
    let home = std::env::home_dir()
        .ok_or_else(|| io::Error::other("test isolation refused: HOME is unavailable"))?;
    if resolved(&home)? == protected {
        return Err(io::Error::other(
            "test isolation refused: set a scratch HOME before booting Bus",
        ));
    }
    if let Some(root) = super::env::bus_data_dir() {
        if root.is_absolute() && resolved(&root)?.starts_with(protected.join(".local/share/bus")) {
            return Err(io::Error::other(
                "test isolation refused: BUS_DATA_DIR points at the protected HOME's Bus data",
            ));
        }
    }
    Ok(())
}
