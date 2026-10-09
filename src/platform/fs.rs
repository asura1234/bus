//! File operations only; callers decide which paths may be written or locked.
use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    super::create_remote_private_dir(path)
}

/// Locks an existing file. Creation and path validation belong to the caller.
pub(crate) fn lock_file(path: &Path) -> io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}

#[derive(Debug)]
pub(crate) struct AtomicWriteError {
    pub(crate) path: PathBuf,
    pub(crate) stage: AtomicWriteStage,
    pub(crate) source: io::Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AtomicWriteStage {
    Parent,
    TempCreate,
    TempWrite,
    TempSync,
    Replace,
    ParentSync,
}

impl AtomicWriteStage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::TempCreate => "temp_create",
            Self::TempWrite => "temp_write",
            Self::TempSync => "temp_sync",
            Self::Replace => "replace",
            Self::ParentSync => "parent_sync",
        }
    }
}

/// Replaces a caller-approved target with a synced, private sibling file.
/// The temporary file is removed after a write, sync or replacement failure.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_detailed(path, bytes).map_err(|error| error.source)
}

/// The same operation with the failed path retained for caller diagnostics.
pub(crate) fn atomic_write_detailed(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    atomic_write_detailed_with_checkpoint(path, bytes, |_| Ok(()))
}

/// Callers can inject a failure before each IO stage without duplicating atomic writes.
pub(crate) fn atomic_write_detailed_with_checkpoint(
    path: &Path,
    bytes: &[u8],
    checkpoint: impl FnMut(AtomicWriteStage) -> io::Result<()>,
) -> Result<(), AtomicWriteError> {
    atomic_write_with_checkpoint(path, |file| file.write_all(bytes), checkpoint)
}

#[cfg(test)]
fn atomic_write_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), AtomicWriteError> {
    atomic_write_with_checkpoint(path, write, |_| Ok(()))
}

fn atomic_write_with_checkpoint(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
    mut checkpoint: impl FnMut(AtomicWriteStage) -> io::Result<()>,
) -> Result<(), AtomicWriteError> {
    let parent = path.parent().ok_or_else(|| AtomicWriteError {
        path: path.to_path_buf(),
        stage: AtomicWriteStage::Parent,
        source: io::Error::other("missing parent"),
    })?;
    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let temp = parent.join(format!(".{stem}.tmp-{}-{sequence}", std::process::id()));
    let mut file = checkpoint(AtomicWriteStage::TempCreate)
        .and_then(|()| create_private_state_file(&temp))
        .map_err(|source| AtomicWriteError {
            path: temp.clone(),
            stage: AtomicWriteStage::TempCreate,
            source,
        })?;
    let result = checkpoint(AtomicWriteStage::TempWrite)
        .and_then(|()| write(&mut file))
        .map_err(|source| (AtomicWriteStage::TempWrite, source))
        .and_then(|()| {
            checkpoint(AtomicWriteStage::TempSync)
                .and_then(|()| file.sync_all())
                .map_err(|source| (AtomicWriteStage::TempSync, source))
        });
    drop(file);
    if let Err((stage, source)) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(AtomicWriteError {
            path: temp,
            stage,
            source,
        });
    }
    if let Err(source) =
        checkpoint(AtomicWriteStage::Replace).and_then(|()| replace_file(&temp, path))
    {
        let _ = std::fs::remove_file(&temp);
        return Err(AtomicWriteError {
            path: path.to_path_buf(),
            stage: AtomicWriteStage::Replace,
            source,
        });
    }
    checkpoint(AtomicWriteStage::ParentSync)
        .and_then(|()| sync_parent_directory(parent))
        .map_err(|source| AtomicWriteError {
            path: parent.to_path_buf(),
            stage: AtomicWriteStage::ParentSync,
            source,
        })
}

#[cfg(not(windows))]
pub(crate) fn create_private_state_file(path: &Path) -> std::io::Result<std::fs::File> {
    super::create_remote_ssh_config_file(path)
}

#[cfg(windows)]
pub(crate) fn create_private_state_file(path: &Path) -> std::io::Result<std::fs::File> {
    super::windows::create_remote_ssh_config_file(path)
}

#[cfg(not(windows))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    super::windows::replace_file(source, destination)
}

#[cfg(not(windows))]
pub(crate) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(crate) fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    // replace_file uses MOVEFILE_WRITE_THROUGH on Windows.
    Ok(())
}

#[cfg(test)]
#[path = "tests/fs_test.rs"]
mod tests;
