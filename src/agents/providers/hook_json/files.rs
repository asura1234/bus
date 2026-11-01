//! Hook configuration path policy over platform file mechanics.
use crate::platform::fs;
use std::{io, path::Path};

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

pub(crate) fn lock(path: &Path) -> io::Result<fs::FileLock> {
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

/// Preserve a matching executable; publish replacements only after fchmod on
/// the exclusively created file. Neither reads nor permission changes follow
/// a bridge symlink, and an existing hardlink's other names stay untouched.
#[cfg(unix)]
pub(super) fn executable_bridge(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(mut file) => {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Err(io::Error::other("Bus bridge is not a regular file"));
            }
            let mut current = Vec::new();
            file.read_to_end(&mut current)?;
            if current == bytes && metadata.permissions().mode() & 0o7777 == 0o700 {
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::atomic_write_with(path, |file| {
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o700))
    })
    .map_err(|error| error.source)
}
