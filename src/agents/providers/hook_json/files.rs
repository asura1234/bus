//! Hook configuration path policy over platform file mechanics.
use crate::platform::fs;
use std::{fs::File, io, path::Path};

pub(crate) fn private_dir(path: &Path) -> io::Result<()> {
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

pub(crate) fn lock(path: &Path) -> io::Result<File> {
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

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
