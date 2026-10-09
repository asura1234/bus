//! Spool path policy over platform-owned creation, locking and replacement.
use crate::platform::fs;
use std::{fs::File, io, path::Path};

pub(super) fn private_dir(path: &Path) -> io::Result<()> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            private_dir(parent)?;
        }
        match fs::create_private_directory(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(io::Error::other("Bus directory is not a real directory"));
    }
    Ok(())
}

fn lock(path: &Path) -> io::Result<File> {
    if !path.exists() {
        match fs::create_private_state_file(path) {
            Ok(file) => file.sync_all()?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("Bus lock is a symlink"));
    }
    fs::lock_file(path)
}

pub(crate) fn append_lock(path: &Path) -> io::Result<File> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match lock(path) {
            Ok(file) => return Ok(file),
            Err(error) if std::time::Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::other("Bus refuses to replace a symlink"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::atomic_write(path, bytes)
}
