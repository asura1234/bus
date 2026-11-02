//! Spool path policy over platform-owned creation, locking and replacement.
use crate::platform::fs;
pub(crate) use crate::platform::fs::guarded_lock_file as try_append_lock;
pub(super) use crate::platform::fs::{guarded_atomic_write as atomic_write, private_dir};
use std::{io, path::Path};

pub(crate) fn append_lock(path: &Path) -> io::Result<fs::FileLock> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match try_append_lock(path) {
            Ok(file) => return Ok(file),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}
